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
    /// AI inference threads: 0 = Auto.
    AiThreads(u32),
    /// The welcome card was read.
    DismissWelcome,
}

#[derive(Debug, Clone, Default)]
pub struct HostSettings {
    pub close_to_tray: bool,
    pub start_minimized: bool,
    pub autostart: Option<bool>,
    /// AI inference threads: 0 = Auto.
    pub ai_threads: u32,
    /// Show the welcome card.
    pub first_run: bool,
}

/// Where the audio side stands (standalone only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioState {
    Starting,
    #[default]
    Running,
    Reconnecting,
    Failed,
}

#[derive(Debug, Clone, Default)]
pub struct AudioStatus {
    pub state: AudioState,
    /// One plain sentence about the problem, empty when fine.
    pub message: String,
    pub virtual_mic: bool,
    /// Audio has arrived from the microphone recently.
    pub mic_alive: bool,
    /// Applications currently hearing the virtual microphone.
    pub listeners: usize,
    /// The microphone the user chose and the one in use (differ while the
    /// chosen one is unplugged); descriptions, not node names.
    pub chosen_mic: Option<String>,
    pub active_mic: Option<String>,
}

/// What this machine offers the AI, for the Settings page.
#[derive(Debug, Clone, Default)]
pub struct HardwareSummary {
    /// "AMD Ryzen 9 5950X · 16 cores, 32 threads · 94 GB memory"
    pub line: String,
    /// Hardware threads: the most the user may pick.
    pub max_threads: u32,
    /// What Auto resolves to right now.
    pub auto_threads: u32,
    /// Threads the loaded voice actually runs with (0 = none loaded).
    pub in_use: u32,
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
    /// Audio health (standalone only).
    fn audio_status(&self) -> Option<AudioStatus> {
        None
    }
    /// Try to start audio again after a failed start.
    fn retry_audio(&self) {}
    /// Cores, threads and the Auto thread count (AI builds only).
    fn hardware(&self) -> Option<HardwareSummary> {
        None
    }
}
