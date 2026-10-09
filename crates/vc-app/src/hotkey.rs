//! Global shortcut through the XDG desktop portal (works on Wayland; KDE and
//! GNOME both implement it). Falls back silently if the portal is missing.

use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::control::Shared;

const TOGGLE_ID: &str = "toggle";

pub async fn run(shared: Arc<Shared>, preferred_trigger: String) {
    if let Err(e) = run_inner(shared, &preferred_trigger).await {
        log::warn!(
            "global shortcut unavailable ({e}). Bind `voice-changer toggle` to a key in \
             System Settings → Shortcuts instead."
        );
    }
}

async fn run_inner(shared: Arc<Shared>, preferred_trigger: &str) -> ashpd::Result<()> {
    let portal = GlobalShortcuts::new().await?;
    let session = portal.create_session(Default::default()).await?;

    // KDE ≥ 6.7.4 leaves shortcuts inert unless they are (re)bound every
    // session, so always bind rather than only listing.
    let shortcuts = [NewShortcut::new(TOGGLE_ID, "Toggle Voice Changer on/off")
        .preferred_trigger(preferred_trigger)];
    let bound = portal
        .bind_shortcuts(&session, &shortcuts, None, Default::default())
        .await?
        .response()?;
    let publish = |trigger: &str| {
        if let Ok(mut t) = shared.hotkey_trigger.lock() {
            *t = Some(trigger.to_string());
        }
        log::info!(
            "global shortcut bound to {}",
            if trigger.is_empty() {
                "(unset — use the Hotkey… button)"
            } else {
                trigger
            }
        );
    };
    for s in bound.shortcuts() {
        publish(s.trigger_description());
    }

    let mut activated = portal.receive_activated().await?;
    let mut changed = portal.receive_shortcuts_changed().await?;
    let mut poll = tokio::time::interval(Duration::from_millis(150));
    loop {
        tokio::select! {
            event = activated.next() => {
                let Some(event) = event else { break };
                if event.shortcut_id() == TOGGLE_ID {
                    shared.toggle();
                }
            }
            change = changed.next() => {
                let Some(change) = change else { break };
                for s in change.shortcuts() {
                    if s.id() == TOGGLE_ID {
                        publish(s.trigger_description());
                    }
                }
            }
            _ = poll.tick() => {
                if shared.quit.load(Ordering::SeqCst) {
                    break;
                }
                if shared.configure_hotkey.swap(false, Ordering::SeqCst) {
                    // Opens the desktop's own key-picker (portal v2; KDE 6.x has it).
                    if let Err(e) = portal.configure_shortcuts(&session, None, Default::default()).await {
                        log::warn!("could not open shortcut configuration: {e}");
                    }
                }
            }
        }
    }
    Ok(())
}
