//! System tray icon (StatusNotifierItem, native on KDE) with the on/off
//! switch, presets and quit.

use ksni::TrayMethods;
use ksni::menu::{CheckmarkItem, MenuItem, StandardItem, SubMenu};
use std::sync::Arc;
use std::time::Duration;

use crate::control::Shared;

pub struct VcTray {
    shared: Arc<Shared>,
    enabled: bool,
}

impl ksni::Tray for VcTray {
    fn id(&self) -> String {
        "voice-changer".into()
    }

    fn title(&self) -> String {
        "Voice Changer".into()
    }

    fn icon_name(&self) -> String {
        // Fallback for trays that ignore pixmaps (the real icon is below).
        if self.enabled {
            "audio-input-microphone".into()
        } else {
            "microphone-sensitivity-muted".into()
        }
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        icon_pixmaps(self.enabled)
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Voice Changer".into(),
            description: if self.enabled {
                "Processing"
            } else {
                "Bypassed"
            }
            .into(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.shared.request_show();
    }

    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        self.shared.toggle();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let presets: Vec<MenuItem<Self>> = vc_core::presets::list_presets()
            .into_iter()
            .map(|entry| {
                let name = entry.preset.name.clone();
                StandardItem {
                    label: name.clone(),
                    activate: Box::new(move |this: &mut Self| {
                        this.shared.apply_preset_named(&name);
                    }),
                    ..Default::default()
                }
                .into()
            })
            .collect();

        vec![
            CheckmarkItem {
                label: "Enabled".into(),
                checked: self.enabled,
                activate: Box::new(|this: &mut Self| {
                    this.enabled = this.shared.toggle();
                }),
                ..Default::default()
            }
            .into(),
            SubMenu {
                label: "Presets".into(),
                submenu: presets,
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Show Window".into(),
                icon_name: "window".into(),
                activate: Box::new(|this: &mut Self| this.shared.request_show()),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|this: &mut Self| this.shared.request_quit()),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// The bundled app icon as ARGB32 pixmaps (so the tray shows it even before
/// the hicolor theme is installed). Dimmed to grey while bypassed.
fn icon_pixmaps(enabled: bool) -> Vec<ksni::Icon> {
    const PNGS: [&[u8]; 3] = [
        include_bytes!("../../../packaging/icons/32.png"),
        include_bytes!("../../../packaging/icons/48.png"),
        include_bytes!("../../../packaging/icons/64.png"),
    ];
    PNGS.iter()
        .filter_map(|png| image::load_from_memory(png).ok())
        .map(|img| {
            let img = img.to_rgba8();
            let (width, height) = (img.width() as i32, img.height() as i32);
            let mut data = Vec::with_capacity((width * height * 4) as usize);
            for px in img.pixels() {
                let [r, g, b, a] = px.0;
                let (r, g, b) = if enabled {
                    (r, g, b)
                } else {
                    let l = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) as u8;
                    (l, l, l)
                };
                data.extend_from_slice(&[a, r, g, b]);
            }
            ksni::Icon {
                width,
                height,
                data,
            }
        })
        .collect()
}

/// Run the tray until quit is requested. Keeps the checkmark/icon in sync
/// with toggles made elsewhere (hotkey, D-Bus, window).
pub async fn run(shared: Arc<Shared>) {
    let tray = VcTray {
        enabled: shared.is_enabled(),
        shared: shared.clone(),
    };
    let handle = match tray.spawn().await {
        Ok(h) => h,
        Err(e) => {
            log::warn!("tray icon unavailable: {e}");
            return;
        }
    };
    let mut last = shared.is_enabled();
    while !shared.quit.load(std::sync::atomic::Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let now = shared.is_enabled();
        if now != last {
            last = now;
            handle.update(|t| t.enabled = now).await;
        }
    }
    handle.shutdown().await;
}
