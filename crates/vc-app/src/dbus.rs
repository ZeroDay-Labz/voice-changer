//! D-Bus control surface: lets `voice-changer toggle` (and KDE custom
//! shortcuts, scripts, stream decks...) drive a running instance.

use std::sync::Arc;

use crate::control::Shared;

pub const BUS_NAME: &str = "org.voicechanger";
pub const OBJECT_PATH: &str = "/org/voicechanger/Control";

pub struct Control {
    pub shared: Arc<Shared>,
}

#[zbus::interface(name = "org.voicechanger.Control")]
impl Control {
    /// Flip processing on/off. Returns the new enabled state.
    fn toggle(&self) -> bool {
        self.shared.toggle()
    }

    fn set_enabled(&self, enabled: bool) {
        self.shared.set_enabled(enabled);
    }

    #[zbus(property)]
    fn enabled(&self) -> bool {
        self.shared.is_enabled()
    }

    /// Apply a factory or user preset by name. Returns false if unknown.
    fn load_preset(&self, name: &str) -> bool {
        self.shared.apply_preset_named(name)
    }

    fn presets(&self) -> Vec<String> {
        vc_core::presets::list_presets()
            .into_iter()
            .map(|e| e.preset.name)
            .collect()
    }

    /// Available microphones as (node.name, description) pairs.
    fn microphones(&self) -> Vec<(String, String)> {
        self.shared
            .devices()
            .into_iter()
            .map(|d| (d.name, d.description))
            .collect()
    }

    /// Current microphone `node.name`, or "" for the system default.
    #[zbus(property)]
    fn microphone(&self) -> String {
        self.shared
            .audio
            .lock()
            .ok()
            .and_then(|a| a.as_ref().and_then(|a| a.capture_target()))
            .unwrap_or_default()
    }

    /// Switch microphone by `node.name` ("" = system default).
    fn set_microphone(&self, name: &str) {
        let target = if name.is_empty() {
            None
        } else {
            Some(name.to_string())
        };
        self.shared.set_capture_target(target);
    }

    /// Play the processed voice to the default output so you can hear yourself.
    #[zbus(property)]
    fn monitor(&self) -> bool {
        self.shared.monitor()
    }

    #[zbus(property)]
    fn set_monitor(&self, on: bool) {
        self.shared.set_monitor(on);
    }

    /// Select an AI voice model by path or by name as listed in `ai_voices`
    /// ("" or "off" disables AI). Returns false if no such voice.
    fn set_ai_voice(&self, voice: &str) -> bool {
        self.shared.set_ai_voice(voice)
    }

    /// Speaker id for multi-speaker voices.
    #[zbus(property)]
    fn ai_speaker(&self) -> i32 {
        self.shared.params.ai_speaker.value()
    }

    #[zbus(property)]
    fn set_ai_speaker(&self, speaker: i32) {
        self.shared
            .set_param(&self.shared.params.ai_speaker, speaker.clamp(0, 15));
    }

    /// Available AI voice models as (name, path) pairs.
    fn ai_voices(&self) -> Vec<(String, String)> {
        self.shared.ai_voices()
    }

    /// Current AI status: "off", "loading", "ready" or "error: ...", plus the voice path.
    fn ai_status(&self) -> (String, String) {
        self.shared.ai_status_text()
    }

    /// Applications recording right now as (name, stream, routed) triples.
    fn applications(&self) -> Vec<(String, String, bool)> {
        self.shared
            .app_streams()
            .into_iter()
            .map(|s| (s.app, s.media, s.routed))
            .collect()
    }

    /// Send the voice to (or release) every recording stream of an application.
    fn route_application(&self, app: &str, on: bool) -> bool {
        self.shared.route_app_named(app, on)
    }

    /// Whether the virtual mic is the session's default microphone.
    #[zbus(property)]
    fn default_source(&self) -> bool {
        self.shared.is_default_source()
    }

    #[zbus(property)]
    fn set_default_source(&self, on: bool) {
        self.shared.set_default_source(on);
    }

    fn show_window(&self) {
        self.shared.request_show();
    }

    fn quit(&self) {
        self.shared.request_quit();
    }
}

#[zbus::proxy(
    interface = "org.voicechanger.Control",
    default_service = "org.voicechanger",
    default_path = "/org/voicechanger/Control",
    gen_blocking = false
)]
pub trait ControlApi {
    fn toggle(&self) -> zbus::Result<bool>;
    fn set_enabled(&self, enabled: bool) -> zbus::Result<()>;
    #[zbus(property)]
    fn enabled(&self) -> zbus::Result<bool>;
    fn load_preset(&self, name: &str) -> zbus::Result<bool>;
    fn presets(&self) -> zbus::Result<Vec<String>>;
    fn microphones(&self) -> zbus::Result<Vec<(String, String)>>;
    #[zbus(property)]
    fn monitor(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn set_monitor(&self, on: bool) -> zbus::Result<()>;
    fn set_ai_voice(&self, voice: &str) -> zbus::Result<bool>;
    fn ai_voices(&self) -> zbus::Result<Vec<(String, String)>>;
    #[zbus(property)]
    fn ai_speaker(&self) -> zbus::Result<i32>;
    #[zbus(property)]
    fn set_ai_speaker(&self, speaker: i32) -> zbus::Result<()>;
    fn ai_status(&self) -> zbus::Result<(String, String)>;
    #[zbus(property)]
    fn microphone(&self) -> zbus::Result<String>;
    fn applications(&self) -> zbus::Result<Vec<(String, String, bool)>>;
    fn route_application(&self, app: &str, on: bool) -> zbus::Result<bool>;
    #[zbus(property)]
    fn default_source(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn set_default_source(&self, on: bool) -> zbus::Result<()>;
    fn set_microphone(&self, name: &str) -> zbus::Result<()>;
    fn show_window(&self) -> zbus::Result<()>;
    fn quit(&self) -> zbus::Result<()>;
}

/// Serve the control interface. Fails with `NameTaken` if another instance runs.
pub async fn serve(shared: Arc<Shared>) -> zbus::Result<zbus::Connection> {
    zbus::connection::Builder::session()?
        .name(BUS_NAME)?
        .serve_at(OBJECT_PATH, Control { shared })?
        .build()
        .await
}

/// Connect to a running instance, if any.
pub async fn client() -> zbus::Result<ControlApiProxy<'static>> {
    let conn = zbus::Connection::session().await?;
    let proxy = ControlApiProxy::new(&conn).await?;
    // Probe so a missing instance fails here rather than on first use.
    proxy.enabled().await?;
    Ok(proxy)
}
