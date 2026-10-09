//! Persistent app settings and the last parameter state.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use vc_core::Preset;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// PipeWire `node.name` of the microphone to capture; `None` = system default.
    pub mic: Option<String>,
    /// Preferred global shortcut in XDG shortcut syntax.
    pub hotkey: String,
    pub start_bypassed: bool,
    pub start_minimized: bool,
    /// Closing the window hides to the tray instead of quitting.
    pub close_to_tray: bool,
    pub last_preset: Option<String>,
    /// "dark" or "light".
    pub theme: String,
    /// Make the virtual mic the session's default microphone while running.
    pub set_as_default: bool,
    /// Last window geometry: x, y, width, height (NaN position = let the desktop choose).
    pub window: Option<[f32; 4]>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            mic: None,
            hotkey: "CTRL+SHIFT+M".into(),
            start_bypassed: false,
            start_minimized: false,
            close_to_tray: false,
            last_preset: None,
            theme: "dark".into(),
            set_as_default: false,
            window: None,
        }
    }
}

fn config_path() -> Option<PathBuf> {
    vc_core::presets::config_dir().map(|d| d.join("config.ron"))
}

fn state_path() -> Option<PathBuf> {
    vc_core::presets::config_dir().map(|d| d.join("state.ron"))
}

impl AppConfig {
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => ron::from_str(&text).unwrap_or_else(|e| {
                log::warn!("ignoring unreadable config {path:?}: {e}");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = config_path().ok_or_else(|| anyhow::anyhow!("no config dir"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, ron::ser::to_string_pretty(self, Default::default())?)?;
        Ok(())
    }
}

/// The full parameter state at last exit, restored on the next start.
pub fn load_state() -> Option<Preset> {
    let text = std::fs::read_to_string(state_path()?).ok()?;
    Preset::from_ron(&text).ok()
}

pub fn save_state(preset: &Preset) -> anyhow::Result<()> {
    let path = state_path().ok_or_else(|| anyhow::anyhow!("no config dir"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, preset.to_ron())?;
    Ok(())
}

// ---------------------------------------------------------------- autostart

const AUTOSTART_FILE: &str = "io.github.zerodaylabz.VoiceChanger.desktop";

fn autostart_path() -> Option<PathBuf> {
    let cfg = vc_core::presets::config_dir()?;
    Some(cfg.parent()?.join("autostart").join(AUTOSTART_FILE))
}

pub fn autostart_enabled() -> bool {
    autostart_path().is_some_and(|p| p.is_file())
}

/// Write or remove the XDG autostart entry (starts minimized to the tray).
pub fn set_autostart(enabled: bool) -> anyhow::Result<()> {
    let path = autostart_path().ok_or_else(|| anyhow::anyhow!("no config dir"))?;
    if !enabled {
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let in_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join("voice-changer").is_file()))
        .unwrap_or(false);
    let exec = if in_path {
        "voice-changer".to_string()
    } else {
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "voice-changer".into())
    };
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=Voice Changer\nComment=Realtime voice changer\nExec={exec} --minimized\nIcon=io.github.zerodaylabz.VoiceChanger\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    );
    std::fs::write(&path, entry)?;
    Ok(())
}
