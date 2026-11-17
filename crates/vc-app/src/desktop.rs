//! Desktop integration where there is no D-Bus (Windows): tray icon, global
//! shortcuts, autostart. The tray and the shortcuts need the window's event
//! loop, so they are created on the main thread just before the window
//! starts; its tick keeps the tray's state current (`poll`).

use std::cell::RefCell;
use std::sync::Arc;

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::control::Shared;

const ID_ENABLED: &str = "enabled";
const ID_MUTE: &str = "mute";
const ID_SHOW: &str = "show";
const ID_QUIT: &str = "quit";
const PRESET_PREFIX: &str = "preset:";

struct Desktop {
    tray: Option<TrayIcon>,
    enabled_item: CheckMenuItem,
    mute_item: CheckMenuItem,
    /// What the tray shows now, to change it only when it differs.
    shown: (bool, bool),
    _hotkeys: Option<GlobalHotKeyManager>,
}

thread_local! {
    static DESKTOP: RefCell<Option<Desktop>> = const { RefCell::new(None) };
}

/// The app icon for the tray: in colour while on, grey while off or muted.
fn icon(active: bool) -> Option<Icon> {
    let img = image::load_from_memory(include_bytes!("../../../packaging/icons/32.png"))
        .ok()?
        .to_rgba8();
    let (width, height) = (img.width(), img.height());
    let mut data = img.into_raw();
    if !active {
        for px in data.as_chunks_mut::<4>().0 {
            let l = (0.299 * px[0] as f32 + 0.587 * px[1] as f32 + 0.114 * px[2] as f32) as u8;
            px[0] = l;
            px[1] = l;
            px[2] = l;
        }
    }
    Icon::from_rgba(data, width, height).ok()
}

fn tooltip(enabled: bool, muted: bool) -> &'static str {
    match (muted, enabled) {
        (true, _) => "Voice Changer: muted",
        (false, true) => "Voice Changer: on",
        (false, false) => "Voice Changer: off (your own voice)",
    }
}

/// Create the tray icon and register the shortcuts. Call on the thread that
/// will run the window. When there is no tray and the window was to start
/// hidden, the window is opened instead: an app with neither cannot be reached.
pub fn start(shared: &Arc<Shared>, started_hidden: bool, toggle_key: &str, mute_key: &str) {
    let enabled = shared.is_enabled();
    let muted = shared.muted();
    let enabled_item = CheckMenuItem::with_id(ID_ENABLED, "Voice changer on", true, enabled, None);
    let mute_item = CheckMenuItem::with_id(ID_MUTE, "Mute", true, muted, None);
    let presets = Submenu::new("Presets", true);
    for entry in vc_core::presets::list_presets() {
        let name = entry.preset.name;
        let _ = presets.append(&MenuItem::with_id(
            format!("{PRESET_PREFIX}{name}"),
            &name,
            true,
            None,
        ));
    }
    let menu = Menu::new();
    let _ = menu.append_items(&[
        &enabled_item,
        &mute_item,
        &presets,
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(ID_SHOW, "Show window", true, None),
        &MenuItem::with_id(ID_QUIT, "Quit", true, None),
    ]);
    let mut builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip(tooltip(enabled, muted));
    if let Some(icon) = icon(enabled && !muted) {
        builder = builder.with_icon(icon);
    }
    let tray = match builder.build() {
        Ok(t) => Some(t),
        Err(e) => {
            log::warn!("tray icon unavailable: {e}");
            if started_hidden {
                shared.request_show();
            }
            None
        }
    };

    let mut toggle_id = None;
    let mut mute_id = None;
    let hotkeys = match GlobalHotKeyManager::new() {
        Ok(manager) => {
            let register = |keys: &str, what: &str| -> Option<u32> {
                if keys.trim().is_empty() {
                    return None;
                }
                let hotkey: HotKey = match keys.parse() {
                    Ok(h) => h,
                    Err(e) => {
                        log::warn!("shortcut {keys:?} for {what} is not understood: {e}");
                        return None;
                    }
                };
                match manager.register(hotkey) {
                    Ok(()) => {
                        log::info!("global shortcut for {what}: {keys}");
                        Some(hotkey.id())
                    }
                    Err(e) => {
                        log::warn!("shortcut {keys} for {what} is taken by another program: {e}");
                        None
                    }
                }
            };
            toggle_id = register(toggle_key, "on/off");
            mute_id = register(mute_key, "mute");
            Some(manager)
        }
        Err(e) => {
            log::warn!("global shortcuts unavailable: {e}");
            None
        }
    };
    if let Ok(mut t) = shared.hotkey_trigger.lock() {
        *t = hotkeys.as_ref().map(|_| {
            if toggle_id.is_some() {
                pretty(toggle_key)
            } else {
                String::new()
            }
        });
    }
    if let Ok(mut t) = shared.mute_hotkey_trigger.lock() {
        *t = mute_id.map(|_| pretty(mute_key));
    }

    // Clicks, menu choices and shortcuts are handled the moment they
    // happen, also while the window is closed.
    {
        let shared = shared.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| match event.id.as_ref() {
            ID_ENABLED => {
                shared.toggle();
            }
            ID_MUTE => shared.set_muted(!shared.muted()),
            ID_SHOW => shared.request_show(),
            ID_QUIT => shared.request_quit(),
            id => {
                if let Some(name) = id.strip_prefix(PRESET_PREFIX) {
                    shared.apply_preset_named(name);
                }
            }
        }));
    }
    {
        let shared = shared.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } => shared.request_show(),
            TrayIconEvent::Click {
                button: MouseButton::Middle,
                button_state: MouseButtonState::Up,
                ..
            } => {
                shared.toggle();
            }
            _ => {}
        }));
    }
    {
        let shared = shared.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state() != HotKeyState::Pressed {
                return;
            }
            if Some(event.id()) == toggle_id {
                shared.toggle();
            } else if Some(event.id()) == mute_id {
                shared.set_muted(!shared.muted());
            }
        }));
    }

    DESKTOP.with(|d| {
        *d.borrow_mut() = Some(Desktop {
            tray,
            enabled_item,
            mute_item,
            shown: (enabled, muted),
            _hotkeys: hotkeys,
        });
    });
}

/// "CTRL+SHIFT+M" as people write it: "Ctrl+Shift+M".
fn pretty(keys: &str) -> String {
    keys.split('+')
        .map(|part| {
            let part = part.trim();
            if part.len() <= 1 {
                part.to_uppercase()
            } else {
                let lower = part.to_lowercase();
                let mut c = lower.chars();
                match c.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Keep the tray in step with changes made elsewhere (window, shortcut,
/// command line). Call from the window's tick.
pub fn poll(shared: &Arc<Shared>) {
    DESKTOP.with(|d| {
        let mut slot = d.borrow_mut();
        let Some(desktop) = slot.as_mut() else { return };
        let now = (shared.is_enabled(), shared.muted());
        if now != desktop.shown {
            desktop.shown = now;
            let (enabled, muted) = now;
            desktop.enabled_item.set_checked(enabled);
            desktop.mute_item.set_checked(muted);
            if let Some(tray) = &desktop.tray {
                let _ = tray.set_icon(icon(enabled && !muted));
                let _ = tray.set_tooltip(Some(tooltip(enabled, muted)));
            }
        }
    });
}

/// Remove the tray icon (otherwise it lingers until the mouse passes over it).
pub fn stop() {
    DESKTOP.with(|d| *d.borrow_mut() = None);
}

// ---------------------------------------------------------------- autostart

#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const RUN_VALUE: &str = "Voice Changer";

#[cfg(windows)]
pub fn autostart_enabled() -> bool {
    winreg::HKCU
        .open_subkey(RUN_KEY)
        .and_then(|k| k.get_value::<String, _>(RUN_VALUE))
        .is_ok()
}

/// Start with Windows (minimized to the tray), for this user only.
#[cfg(windows)]
pub fn set_autostart(enabled: bool) -> anyhow::Result<()> {
    let (key, _) = winreg::HKCU.create_subkey(RUN_KEY)?;
    if enabled {
        let exe = std::env::current_exe()?;
        key.set_value(RUN_VALUE, &format!("\"{}\" --minimized", exe.display()))?;
    } else if key.get_value::<String, _>(RUN_VALUE).is_ok() {
        key.delete_value(RUN_VALUE)?;
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn autostart_enabled() -> bool {
    false
}

#[cfg(not(windows))]
pub fn set_autostart(_enabled: bool) -> anyhow::Result<()> {
    anyhow::bail!("starting with the desktop is not supported on this system yet")
}
