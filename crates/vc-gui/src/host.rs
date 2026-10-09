//! What a host (standalone app, plugin editor) offers the shared UI.

use nice_plug::context::gui::GuiContext;
use vc_core::{Meters, VcParams};

#[derive(Debug, Clone, Copy, Default)]
pub struct Capabilities {
    pub standalone: bool,
    pub monitor: bool,
    pub microphone: bool,
    pub hotkey: bool,
    pub settings: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HostStats {
    pub underruns: u64,
    pub process_max_us: u64,
    pub quantum_us: u64,
}

#[derive(Debug, Clone)]
pub enum HostSetting {
    CloseToTray(bool),
    StartMinimized(bool),
    Autostart(bool),
    Theme(crate::theme::Mode),
}

#[derive(Debug, Clone, Default)]
pub struct HostSettings {
    pub close_to_tray: bool,
    pub start_minimized: bool,
    pub autostart: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub description: String,
}

/// An application that is recording right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppStream {
    pub id: u32,
    pub app: String,
    pub media: String,
    pub routed: bool,
}

pub trait Host {
    fn params(&self) -> &VcParams;
    fn gui(&self) -> &GuiContext;
    fn meters(&self) -> &Meters;
    fn theme_mode(&self) -> crate::theme::Mode;
    fn capabilities(&self) -> Capabilities;
    fn latency_ms(&self) -> f32;
    #[cfg(feature = "ai")]
    fn ai_status(&self) -> Option<std::sync::Arc<vc_core::ai::AiStatus>> {
        None
    }
    fn stats(&self) -> Option<HostStats> {
        None
    }
    fn devices(&self) -> Vec<Device> {
        Vec::new()
    }
    fn capture_target(&self) -> Option<String> {
        None
    }
    fn set_capture_target(&self, _target: Option<String>) {}
    fn monitor(&self) -> Option<bool> {
        None
    }
    /// Recording applications the voice can be sent to (standalone only).
    fn app_streams(&self) -> Vec<AppStream> {
        Vec::new()
    }
    fn set_app_route(&self, _stream_id: u32, _on: bool) {}
    /// `None` = not supported; `Some(on)` = whether we are the default microphone.
    fn default_source(&self) -> Option<bool> {
        None
    }
    fn set_default_source(&self, _on: bool) {}
    /// Which ONNX Runtime the AI uses, for the Settings page.
    fn runtime_description(&self) -> Option<String> {
        None
    }
    fn set_monitor(&self, _on: bool) {}
    /// `None` = no hotkey support; `Some(None)` = supported but unset.
    fn hotkey(&self) -> Option<Option<String>> {
        None
    }
    fn configure_hotkey(&self) {}
    fn settings(&self) -> HostSettings {
        HostSettings::default()
    }
    fn apply_setting(&self, _setting: HostSetting) {}
    fn log_lines(&self) -> Vec<String> {
        Vec::new()
    }
    fn open_data_folder(&self) {}
    fn quit(&self) {}
}
