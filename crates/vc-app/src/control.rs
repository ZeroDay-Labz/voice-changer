//! State shared between the audio thread, the window, the tray, the hotkey
//! listener and the D-Bus server.

use nice_plug::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use vc_core::{Meters, Preset, VcParams};

use crate::config::AppConfig;
use crate::pw::{AppStream, AudioConfig, AudioHealth, AudioState, AudioThread, DeviceInfo, Stats};

pub struct Shared {
    pub params: Arc<VcParams>,
    pub sample_rate: f32,
    pub quantum: u32,
    /// `None` while the audio side could not be started (see `audio_error`).
    pub audio: Mutex<Option<AudioThread>>,
    /// What to spawn again when the user presses Retry.
    pub audio_config: AudioConfig,
    /// Why audio is not running, in one sentence for the window.
    pub audio_error: Mutex<Option<String>>,
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
    /// Test bench: the clip being recorded, the clip being replayed, and
    /// the names of the clips on disk.
    bench_recording: Mutex<Option<String>>,
    bench_playing: Mutex<Option<String>>,
    bench_clips: Mutex<Vec<String>>,
}

/// How long a bench clip is, in seconds.
const BENCH_SECONDS: usize = 8;

/// `~/.local/share/voice-changer/bench`
fn bench_dir() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("", "echo", "voice-changer").map(|d| d.data_dir().join("bench"))
}

fn bench_clips_on_disk() -> Vec<String> {
    let Some(dir) = bench_dir() else {
        return Vec::new();
    };
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension().is_some_and(|x| x == "wav"))
                .then(|| p.file_stem().map(|s| s.to_string_lossy().to_string()))
                .flatten()
        })
        .collect();
    names.sort();
    names
}

impl Shared {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        params: Arc<VcParams>,
        sample_rate: f32,
        quantum: u32,
        audio: Result<AudioThread, String>,
        audio_config: AudioConfig,
        stats: Arc<Stats>,
        meters: Arc<Meters>,
        config: AppConfig,
    ) -> Arc<Self> {
        let (audio, error) = match audio {
            Ok(a) => (Some(a), None),
            Err(e) => (None, Some(e)),
        };
        Arc::new(Self {
            params,
            sample_rate,
            quantum,
            audio: Mutex::new(audio),
            audio_config,
            audio_error: Mutex::new(error),
            stats,
            meters,
            config: Mutex::new(config),
            quit: AtomicBool::new(false),
            show_window: AtomicBool::new(false),
            configure_hotkey: AtomicBool::new(false),
            hotkey_trigger: Mutex::new(None),
            current_preset: Mutex::new(None),
            bench_recording: Mutex::new(None),
            bench_playing: Mutex::new(None),
            bench_clips: Mutex::new(bench_clips_on_disk()),
        })
    }

    /// Record the raw microphone for a few seconds as bench clip `name`.
    pub fn bench_record(&self, name: &str) {
        let name: String = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if name.is_empty() {
            return;
        }
        if let Ok(audio) = self.audio.lock()
            && let Some(a) = audio.as_ref()
        {
            a.bench.play(None);
            a.bench.record(self.sample_rate as usize * BENCH_SECONDS);
            if let Ok(mut p) = self.bench_playing.lock() {
                *p = None;
            }
            if let Ok(mut r) = self.bench_recording.lock() {
                *r = Some(name);
            }
        }
    }

    /// Replay a bench clip in place of the microphone, or stop with `None`.
    /// The clip is heard through "Hear myself" only.
    pub fn bench_play(&self, name: Option<&str>) -> bool {
        let clip = match name {
            None => None,
            Some(name) => {
                let Some(path) = bench_dir().map(|d| d.join(format!("{name}.wav"))) else {
                    return false;
                };
                match self.bench_load(&path) {
                    Some(c) => Some(c),
                    None => return false,
                }
            }
        };
        self.bench_play_clip(name.map(str::to_string), clip)
    }

    /// Replay any WAV file as a bench clip (for tests).
    pub fn bench_play_file(&self, path: &std::path::Path) -> bool {
        match self.bench_load(path) {
            Some(clip) => self.bench_play_clip(Some(path.display().to_string()), Some(clip)),
            None => false,
        }
    }

    fn bench_load(&self, path: &std::path::Path) -> Option<Arc<Vec<f32>>> {
        match vc_core::wav::read_mono(path) {
            Ok((audio, rate)) if rate as f32 == self.sample_rate => Some(Arc::new(audio)),
            Ok((_, rate)) => {
                log::warn!(
                    "{}: {rate} Hz, need {} Hz",
                    path.display(),
                    self.sample_rate
                );
                None
            }
            Err(e) => {
                log::warn!("could not load the bench clip: {e:#}");
                None
            }
        }
    }

    fn bench_play_clip(&self, name: Option<String>, clip: Option<Arc<Vec<f32>>>) -> bool {
        let Ok(audio) = self.audio.lock() else {
            return false;
        };
        let Some(a) = audio.as_ref() else {
            return false;
        };
        let starting = clip.is_some();
        a.bench.play(clip);
        drop(audio);
        if let Ok(mut p) = self.bench_playing.lock() {
            *p = name;
        }
        // The clip goes to "Hear myself" only: without it nothing is heard.
        if starting && !self.monitor() {
            self.set_monitor(true);
        }
        true
    }

    /// What the bench is doing; also finishes a recording (writes the file)
    /// once the audio side has collected it. Cheap enough for every frame.
    pub fn bench_state(&self) -> vc_gui::BenchState {
        let mut progress = None;
        if let Ok(audio) = self.audio.lock()
            && let Some(a) = audio.as_ref()
        {
            if let Some(done) = a.bench.take_recording() {
                let name = self.bench_recording.lock().ok().and_then(|mut r| r.take());
                if let (Some(name), Some(dir)) = (name, bench_dir()) {
                    let path = dir.join(format!("{name}.wav"));
                    match vc_core::wav::write_mono(&path, &done, self.sample_rate as usize) {
                        Ok(()) => log::info!("bench clip saved: {}", path.display()),
                        Err(e) => log::warn!("could not save the bench clip: {e:#}"),
                    }
                    if let Ok(mut clips) = self.bench_clips.lock() {
                        *clips = bench_clips_on_disk();
                    }
                }
            }
            progress = a.bench.record_progress();
            if !a.bench.playing()
                && let Ok(mut p) = self.bench_playing.lock()
            {
                *p = None;
            }
        }
        let recording = self
            .bench_recording
            .lock()
            .ok()
            .and_then(|r| r.clone())
            .zip(progress);
        vc_gui::BenchState {
            clips: self
                .bench_clips
                .lock()
                .map(|c| c.clone())
                .unwrap_or_default(),
            recording,
            playing: self.bench_playing.lock().ok().and_then(|p| p.clone()),
        }
    }

    pub fn is_enabled(&self) -> bool {
        !self.params.bypass.value()
    }

    /// Try to start the audio side again (after a failed start).
    pub fn retry_audio(&self) -> bool {
        if self.audio.lock().map(|a| a.is_some()).unwrap_or(true) {
            return true;
        }
        match AudioThread::spawn(
            self.audio_config.clone(),
            self.params.clone(),
            self.stats.clone(),
            self.meters.clone(),
        ) {
            Ok(a) => {
                // `stats`/`meters` are per-thread Arcs; the window reads
                // through `audio` for those from now on.
                if let Ok(mut slot) = self.audio.lock() {
                    *slot = Some(a);
                }
                if let Ok(mut e) = self.audio_error.lock() {
                    *e = None;
                }
                let want_default = self
                    .config
                    .lock()
                    .map(|c| c.set_as_default)
                    .unwrap_or(false);
                if want_default {
                    self.set_default_source(true);
                }
                let mic = self.config.lock().ok().and_then(|c| c.mic.clone());
                if mic.is_some() {
                    self.set_capture_target(mic);
                }
                log::info!("audio started");
                true
            }
            Err(e) => {
                if let Ok(mut err) = self.audio_error.lock() {
                    *err = Some(format!("{e:#}"));
                }
                false
            }
        }
    }

    /// The audio side's state, with the start-up failure folded in.
    pub fn audio_health(&self) -> AudioHealth {
        let running = self
            .audio
            .lock()
            .ok()
            .and_then(|a| a.as_ref().map(|a| a.health()));
        match running {
            Some(h) => h,
            None => AudioHealth {
                state: AudioState::Failed,
                message: self
                    .audio_error
                    .lock()
                    .ok()
                    .and_then(|e| e.clone())
                    .unwrap_or_else(|| "Audio is not running.".into()),
                ..AudioHealth::default()
            },
        }
    }

    /// Applications currently linked to the virtual microphone.
    pub fn listeners(&self) -> usize {
        self.audio
            .lock()
            .ok()
            .and_then(|a| a.as_ref().map(|a| a.listeners()))
            .unwrap_or(0)
    }

    /// The microphone in use right now (None = system default).
    pub fn active_target(&self) -> Option<String> {
        self.audio
            .lock()
            .ok()
            .and_then(|a| a.as_ref().and_then(|a| a.active_target()))
    }

    /// AI inference threads: 0 = Auto. Saved and applied immediately.
    pub fn set_ai_threads(&self, threads: u32) {
        self.params.set_ai_threads(threads);
        if let Ok(mut c) = self.config.lock() {
            c.ai_threads = threads;
        }
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
            vc_core::ai::library::apply_voice_settings(&v.path, &self.params, |ptr, n| {
                // SAFETY: `ptr` points into `self.params`.
                unsafe {
                    ptr._internal_set_normalized_value(n);
                    ptr._internal_update_smoother(self.sample_rate, true);
                }
            });
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
