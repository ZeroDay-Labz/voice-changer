//! State shared between the audio thread, the window, the tray, the hotkey
//! listener and the D-Bus server.

use nice_plug::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use vc_core::{Meters, Preset, VcParams};

use crate::config::AppConfig;
use crate::pw::{AppStream, AudioThread, DeviceInfo, Stats};

pub struct Shared {
    pub params: Arc<VcParams>,
    pub sample_rate: f32,
    pub quantum: u32,
    pub audio: Mutex<Option<AudioThread>>,
    pub stats: Arc<Stats>,
    pub meters: Arc<Meters>,
    /// Persistent settings; the Settings page edits and saves them.
    pub config: Mutex<AppConfig>,
    pub quit: AtomicBool,
    pub show_window: AtomicBool,
    /// Window → hotkey task: open the desktop's shortcut configuration dialog.
    pub configure_hotkey: AtomicBool,
    /// Hotkey task → window: human-readable current trigger ("" = unset, None = portal unavailable).
    pub hotkey_trigger: Mutex<Option<String>>,
    /// Name of the last preset applied (for the window and the saved state).
    pub current_preset: Mutex<Option<String>>,
}

impl Shared {
    pub fn new(
        params: Arc<VcParams>,
        sample_rate: f32,
        quantum: u32,
        audio: AudioThread,
        config: AppConfig,
    ) -> Arc<Self> {
        let stats = audio.stats.clone();
        let meters = audio.meters.clone();
        Arc::new(Self {
            params,
            sample_rate,
            quantum,
            audio: Mutex::new(Some(audio)),
            stats,
            meters,
            config: Mutex::new(config),
            quit: AtomicBool::new(false),
            show_window: AtomicBool::new(false),
            configure_hotkey: AtomicBool::new(false),
            hotkey_trigger: Mutex::new(None),
            current_preset: Mutex::new(None),
        })
    }

    pub fn is_enabled(&self) -> bool {
        !self.params.bypass.value()
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.set_param(&self.params.bypass, !enabled);
        log::info!(
            "voice changer {}",
            if enabled { "ON" } else { "OFF (bypassed)" }
        );
    }

    /// Flip bypass; returns the new enabled state.
    pub fn toggle(&self) -> bool {
        let enabled = !self.is_enabled();
        self.set_enabled(enabled);
        enabled
    }

    /// Set a parameter from any non-GUI thread (no host involved).
    pub fn set_param<P: Param>(&self, param: &P, value: P::Plain) {
        let normalized = param.preview_normalized(value);
        self.set_param_normalized(param.as_ptr(), normalized);
    }

    pub fn set_param_normalized(&self, ptr: ParamPtr, normalized: f32) {
        // SAFETY: every `ParamPtr` handed to us points into `self.params`,
        // which lives as long as `Shared`.
        unsafe {
            ptr._internal_set_normalized_value(normalized);
            ptr._internal_update_smoother(self.sample_rate, false);
        }
    }

    pub fn apply_preset(&self, preset: &Preset) {
        preset.apply(&self.params, |ptr, normalized| {
            self.set_param_normalized(ptr, normalized)
        });
        if let Ok(mut cur) = self.current_preset.lock() {
            *cur = Some(preset.name.clone());
        }
        log::info!("preset: {}", preset.name);
    }

    /// Apply a preset by (case-insensitive) name. Returns false if unknown.
    pub fn apply_preset_named(&self, name: &str) -> bool {
        let found = vc_core::presets::list_presets()
            .into_iter()
            .find(|e| e.preset.name.eq_ignore_ascii_case(name));
        match found {
            Some(entry) => {
                self.apply_preset(&entry.preset);
                true
            }
            None => false,
        }
    }

    pub fn devices(&self) -> Vec<DeviceInfo> {
        self.audio
            .lock()
            .ok()
            .and_then(|a| a.as_ref().map(|a| a.devices()))
            .unwrap_or_default()
    }

    pub fn monitor(&self) -> bool {
        self.audio
            .lock()
            .ok()
            .and_then(|a| a.as_ref().map(|a| a.monitor()))
            .unwrap_or(false)
    }

    pub fn set_monitor(&self, on: bool) {
        if let Ok(a) = self.audio.lock()
            && let Some(a) = a.as_ref()
        {
            a.set_monitor(on);
        }
    }

    pub fn app_streams(&self) -> Vec<AppStream> {
        self.audio
            .lock()
            .ok()
            .and_then(|a| a.as_ref().map(|a| a.app_streams()))
            .unwrap_or_default()
    }

    pub fn route_app(&self, stream_id: u32, on: bool) {
        if let Ok(a) = self.audio.lock()
            && let Some(a) = a.as_ref()
        {
            a.route_app(stream_id, on);
        }
    }

    /// Route every recording stream of an application by (case-insensitive) name.
    pub fn route_app_named(&self, app: &str, on: bool) -> bool {
        let streams: Vec<_> = self
            .app_streams()
            .into_iter()
            .filter(|s| s.app.eq_ignore_ascii_case(app))
            .collect();
        for s in &streams {
            self.route_app(s.id, on);
        }
        !streams.is_empty()
    }

    pub fn is_default_source(&self) -> bool {
        self.audio
            .lock()
            .ok()
            .and_then(|a| a.as_ref().map(|a| a.is_default_source()))
            .unwrap_or(false)
    }

    pub fn set_default_source(&self, on: bool) {
        if let Ok(a) = self.audio.lock()
            && let Some(a) = a.as_ref()
        {
            a.set_default_source(on);
        }
        if let Ok(mut c) = self.config.lock() {
            c.set_as_default = on;
        }
    }

    pub fn set_capture_target(&self, target: Option<String>) {
        if let Ok(a) = self.audio.lock()
            && let Some(a) = a.as_ref()
        {
            a.set_capture_target(target);
        }
    }

    /// Pick an AI voice by path or display name; "" / "off" turns AI off.
    pub fn set_ai_voice(&self, voice: &str) -> bool {
        if voice.is_empty()
            || voice.eq_ignore_ascii_case("off")
            || voice.eq_ignore_ascii_case("none")
        {
            self.set_param(&self.params.ai_enabled, false);
            return true;
        }
        #[cfg(feature = "ai")]
        {
            let found = vc_core::ai::list_voices()
                .into_iter()
                .find(|v| v.path.to_string_lossy() == voice || v.name.eq_ignore_ascii_case(voice));
            let Some(v) = found else { return false };
            self.params.set_ai_voice(&v.path.to_string_lossy());
            self.set_param(
                &self.params.ai_speaker,
                vc_core::ai::library::default_speaker_for(&v.path),
            );
            self.set_param(&self.params.ai_enabled, true);
            log::info!("AI voice: {}", v.name);
            true
        }
        #[cfg(not(feature = "ai"))]
        {
            false
        }
    }

    pub fn ai_voices(&self) -> Vec<(String, String)> {
        #[cfg(feature = "ai")]
        {
            vc_core::ai::list_voices()
                .into_iter()
                .map(|v| (v.name, v.path.to_string_lossy().to_string()))
                .collect()
        }
        #[cfg(not(feature = "ai"))]
        {
            Vec::new()
        }
    }

    pub fn ai_status_text(&self) -> (String, String) {
        #[cfg(feature = "ai")]
        {
            use vc_core::ai::AiState;
            let status = self
                .audio
                .lock()
                .ok()
                .and_then(|a| a.as_ref().map(|a| a.ai_status.clone()));
            let text = match status {
                None => "unavailable".to_string(),
                Some(s) => match s.state() {
                    AiState::Off => "off".into(),
                    AiState::Loading => "loading".into(),
                    AiState::Error => format!("error: {}", s.message()),
                    AiState::Ready => {
                        if self.params.ai_enabled.value() {
                            format!(
                                "ready ({:.0} ms/block, {:.0}% load, {} dropouts)",
                                s.infer_ms.load(Ordering::Relaxed),
                                s.load_factor() * 100.0,
                                s.dropouts.load(Ordering::Relaxed)
                            )
                        } else {
                            "ready (disabled)".into()
                        }
                    }
                },
            };
            (text, self.params.ai_voice())
        }
        #[cfg(not(feature = "ai"))]
        {
            ("unavailable".into(), String::new())
        }
    }

    pub fn request_quit(&self) {
        self.quit.store(true, Ordering::SeqCst);
    }

    pub fn request_show(&self) {
        self.show_window.store(true, Ordering::SeqCst);
    }
}
