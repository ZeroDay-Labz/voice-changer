//! Voice Changer user interface (iced). One `Model` + `Message` + `view`
//! serves the standalone app and the plugin editor; host-specific abilities
//! (microphone list, monitor, settings…) come through the [`Host`] trait.

#![cfg_attr(not(feature = "ai"), allow(unused))]

pub mod help;
pub mod host;
pub mod pages;
pub mod theme;
pub mod widgets;

use iced_core::Length;
use iced_widget::{button, column, container, row, scrollable, text, tooltip};
use nice_plug::prelude::*;
use std::time::Instant;
use vc_core::{Preset, PresetEntry};

pub use host::{
    AppStream, AudioState, AudioStatus, BenchState, Capabilities, Device, HardwareSummary, Host,
    HostSetting, HostSettings, HostStats,
};
pub use iced_runtime::Task;
pub use theme::Choice as ThemeChoice;
pub use theme::Mode;

pub type Element<'a> = iced_core::Element<'a, Message, iced_core::Theme, iced_renderer::Renderer>;

pub const MIN_WIDTH: f32 = 960.0;
pub const MIN_HEIGHT: f32 = 700.0;
pub const DEFAULT_WIDTH: f32 = 1120.0;
pub const DEFAULT_HEIGHT: f32 = 800.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    Home,
    Voices,
    Effects,
    Mixer,
    Settings,
}

impl Page {
    pub const ALL: [Page; 5] = [
        Page::Home,
        Page::Voices,
        Page::Effects,
        Page::Mixer,
        Page::Settings,
    ];
    /// Each page has its own highlight colour so the UI is not all one green.
    pub fn accent(self) -> iced_core::Color {
        match self {
            Page::Home => theme::ACCENT,
            Page::Voices => theme::INDIGO,
            Page::Effects => theme::VIOLET,
            Page::Mixer => theme::SKY,
            Page::Settings => iced_core::Color::from_rgb8(150, 158, 172),
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Page::Home => "Home",
            Page::Voices => "Voices",
            Page::Effects => "Effects",
            Page::Mixer => "Mixer",
            Page::Settings => "Settings",
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    Go(Page),
    /// Parameter gestures (normalized 0..1).
    ParamBegin(ParamPtr),
    /// Begin a gesture and set the first value in one go (click on a slider).
    ParamPress(ParamPtr, f32),
    ParamSet(ParamPtr, f32),
    ParamEnd(ParamPtr),
    ParamReset(ParamPtr),
    ParamBool(ParamPtr, bool),
    /// Begin + set + end in one go (pickers, toggles).
    ParamJump(ParamPtr, f32),
    ToggleFavorite(String),
    TogglePower,
    ToggleMute,
    ToggleMonitor,
    ApplyPreset(String),
    SearchChanged(String),
    SaveNameChanged(String),
    StartSave,
    CancelSave,
    SavePreset,
    /// Show or hide the rename / export / delete buttons on the presets.
    TogglePresetEdit,
    AskDeletePreset(String),
    CancelDeletePreset,
    DeletePreset(String),
    StartRenamePreset(String),
    RenameChanged(String),
    CommitRename,
    /// Store the current settings in an existing user preset.
    UpdatePreset(String),
    /// Ask the host for a file dialog; the answer comes back as `FilePicked`.
    PickFile(FileRequest),
    FilePicked(FileRequest, Option<std::path::PathBuf>),
    /// A file was dropped on the window.
    FileDropped(std::path::PathBuf),
    RefreshLibrary,
    SelectMic(Option<String>),
    SelectOutput(Option<String>),
    ConfigureHotkey,
    Setting(HostSetting),
    OpenDataFolder,
    Quit,
    DismissToast,
    // AI library
    ImportUrlChanged(String),
    ImportNameChanged(String),
    StartImport,
    CancelImport,
    DismissImport,
    UseVoice(String),
    AskDeleteVoice(String),
    CancelDeleteVoice,
    ConfirmDeleteVoice,
    SelectVoice(String),
    SelectDevice(String),
    DownloadBaseModels,
    /// Test bench: record the named clip / replay it (or stop with `None`).
    BenchRecord(String),
    BenchPlay(Option<String>),
    /// Run the quality check on an installed voice (by path).
    CheckVoice(String),
    RouteApp(u32, bool),
    SetDefaultSource(bool),
    RefreshApps,
    /// How many speakers a voice has (RVC files do not record it).
    SetVoiceSpeakers(String, u32),
    /// Pin which speaker a voice starts with.
    SetDefaultSpeaker(String, u32),
    SpeakerNameChanged(String, u32, String),
    SaveSpeakerName(String, u32),
    ResetMany(Vec<ParamPtr>),
    Escape,
    FocusSearch,
    /// Start the audio side again after a failed start.
    RetryAudio,
    DismissWelcome,
    /// Show or hide the less-used AI settings on the Home page.
    ToggleAiAdvanced,
}

/// What a file dialog is wanted for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileRequest {
    /// Open: a preset file to add.
    ImportPreset,
    /// Save: where to write the named preset.
    ExportPreset(String),
    /// Open: a voice model to add.
    ImportVoice,
}

pub fn search_id() -> iced_core::widget::Id {
    iced_core::widget::Id::new("preset-search")
}

/// Everything the UI remembers between frames (not audio state).
pub struct Model {
    pub page: Page,
    pub presets: Vec<PresetEntry>,
    pub current_preset: Option<String>,
    pub search: String,
    pub save_name: String,
    pub saving: bool,
    /// The preset cards show their rename / export / delete buttons.
    pub editing_presets: bool,
    /// The preset waiting for "really delete?".
    pub confirm_delete_preset: Option<String>,
    /// The preset being renamed and the name typed so far.
    pub renaming: Option<(String, String)>,
    /// The user preset the current settings started from, offered for
    /// "Update" once they have been changed.
    pub last_user_preset: Option<String>,
    /// The Home page shows the less-used AI settings.
    pub ai_advanced: bool,
    /// 0..1, going round while the window is open: drives the "working on
    /// it" bar of steps whose length is not known.
    pub phase: f32,
    pub toast: Option<(String, bool, Instant)>,
    pub meters: widgets::meter::Levels,
    /// The test bench, refreshed every tick (`None` = this host has none).
    pub bench: Option<host::BenchState>,
    #[cfg(feature = "ai")]
    pub voices: Vec<vc_core::ai::VoiceInfo>,
    #[cfg(feature = "ai")]
    pub import: Option<std::sync::Arc<vc_core::ai::ImportStatus>>,
    pub import_url: String,
    pub import_name: String,
    pub confirm_delete: Option<String>,
    /// In-progress speaker name edits, keyed by (voice path, speaker).
    pub speaker_edits: std::collections::HashMap<(String, u32), String>,
    pub favorites: Vec<String>,
    /// Slow-changing host data, refreshed every couple of seconds (never in `view`).
    pub cache: Cache,
    last_tick: Instant,
    last_refresh: Instant,
    last_status: Instant,
}

/// Values that are expensive or non-trivial to compute, snapshotted for `view`.
#[derive(Default, Clone)]
pub struct Cache {
    pub devices: Vec<Device>,
    pub capture_target: Option<String>,
    pub outputs: Vec<Device>,
    pub output_target: Option<String>,
    pub app_streams: Vec<AppStream>,
    pub default_source: Option<bool>,
    pub base_models_present: bool,
    /// The cough and burp blocker's classifier is downloaded.
    pub blocker_model_present: bool,
    pub voices_dir: String,
    /// (setting key, label) for the compute picker.
    pub compute_options: Vec<(String, String)>,
    pub gpu_note: Option<String>,
    pub runtime: Option<String>,
    /// Audio health, refreshed twice a second.
    pub audio: Option<AudioStatus>,
    pub hardware: Option<HardwareSummary>,
    pub settings: HostSettings,
}

impl Default for Model {
    fn default() -> Self {
        Self::new()
    }
}

impl Model {
    pub fn new() -> Self {
        Self {
            page: Page::Home,
            presets: vc_core::presets::list_presets(),
            current_preset: None,
            search: String::new(),
            save_name: String::new(),
            saving: false,
            editing_presets: false,
            confirm_delete_preset: None,
            renaming: None,
            last_user_preset: None,
            ai_advanced: false,
            phase: 0.0,
            toast: None,
            meters: widgets::meter::Levels::default(),
            bench: None,
            #[cfg(feature = "ai")]
            voices: vc_core::ai::library::voices(),
            #[cfg(feature = "ai")]
            import: None,
            import_url: String::new(),
            import_name: String::new(),
            confirm_delete: None,
            speaker_edits: Default::default(),
            favorites: load_favorites(),
            cache: Cache::default(),
            last_tick: Instant::now(),
            last_refresh: Instant::now() - std::time::Duration::from_secs(10),
            last_status: Instant::now() - std::time::Duration::from_secs(10),
        }
    }

    /// The cheap, fast-changing part of the host snapshot.
    pub fn refresh_status(&mut self, host: &dyn Host) {
        self.last_status = Instant::now();
        self.cache.audio = host.audio_status();
    }

    pub fn refresh(&mut self) {
        self.presets = vc_core::presets::list_presets();
        #[cfg(feature = "ai")]
        {
            self.voices = vc_core::ai::library::voices();
        }
        self.last_refresh = Instant::now() - std::time::Duration::from_secs(10);
    }

    /// Snapshot host data for the next frames. Called from `update`, never `view`.
    pub fn refresh_cache(&mut self, host: &dyn Host) {
        self.last_refresh = Instant::now();
        self.cache.devices = host.devices();
        self.cache.capture_target = host.capture_target();
        self.cache.outputs = host.output_devices();
        self.cache.output_target = host.output_target();
        self.cache.app_streams = host.app_streams();
        self.cache.default_source = host.default_source();
        self.cache.runtime = host.runtime_description();
        self.cache.audio = host.audio_status();
        self.cache.hardware = host.hardware();
        self.cache.settings = host.settings();
        #[cfg(feature = "ai")]
        {
            use vc_core::ai::compute;
            self.cache.base_models_present = vc_core::ai::library::base_models_present();
            self.cache.blocker_model_present = vc_core::ai::events::model_path().is_some();
            self.cache.voices_dir = vc_core::ai::voices_dir()
                .map(|d| d.display().to_string())
                .unwrap_or_default();
            // Never block the window on `rocminfo`/`nvidia-smi`: until the
            // background probe is done the picker only offers Auto.
            let Some(gpus) = compute::gpus_if_probed() else {
                self.cache.compute_options =
                    vec![("auto".to_string(), "Auto (detecting…)".to_string())];
                self.cache.gpu_note = None;
                return;
            };
            let mut opts = vec![
                (
                    "auto".to_string(),
                    match compute::resolve("auto") {
                        compute::Backend::Cpu => "Auto (CPU)".to_string(),
                        compute::Backend::Rocm { device }
                        | compute::Backend::Cuda { device }
                        | compute::Backend::DirectMl { device } => {
                            format!("Auto (GPU {device})")
                        }
                    },
                ),
                ("cpu".to_string(), "CPU".to_string()),
            ];
            for g in &gpus {
                let backend = match g.api {
                    "CUDA" => compute::Backend::Cuda { device: g.index },
                    "DirectML" => compute::Backend::DirectMl { device: g.index },
                    _ => compute::Backend::Rocm { device: g.index },
                };
                opts.push((
                    compute::setting_string(Some(backend)),
                    if g.api == "DirectML" {
                        "Graphics card (experimental)".to_string()
                    } else {
                        format!("GPU {} · {} ({})", g.index, g.name, g.api)
                    },
                ));
            }
            self.cache.compute_options = opts;
            self.cache.gpu_note = compute::gpu_note();
        }
    }

    pub fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toast = Some((text.into(), error, Instant::now()));
    }

    /// After a control was changed: the preset named in the window is only
    /// still "the preset" while the settings match it. Otherwise the window
    /// says Custom, so it never claims a sound that is not what is heard.
    pub fn preset_still_applies(&mut self, host: &dyn Host) {
        let Some(name) = &self.current_preset else {
            return;
        };
        let matches = self
            .presets
            .iter()
            .find(|e| e.preset.name == *name)
            .is_some_and(|e| e.preset.matches(host.params()));
        if !matches {
            self.current_preset = None;
        }
    }
}

/// Select a voice and start it on its pinned speaker.
#[cfg(feature = "ai")]
fn select_voice(host: &dyn Host, path: &str) {
    host.params().set_ai_voice(path);
    if !path.is_empty() {
        let voice = std::path::Path::new(path);
        let idx = vc_core::ai::library::default_speaker_for(voice);
        let p = &host.params().ai_speaker;
        gesture(host, p.as_ptr(), p.preview_normalized(idx));
        // Pitch, index and loudness settings last used with this voice.
        vc_core::ai::library::apply_voice_settings(voice, host.params(), |ptr, n| {
            gesture(host, ptr, n)
        });
    }
}

/// A per-voice setting was just changed by the user: remember it with the
/// voice, so switching away and back restores it.
#[cfg(feature = "ai")]
fn remember_voice_setting(host: &dyn Host, ptr: ParamPtr) {
    let p = host.params();
    let voice = p.ai_voice();
    if voice.is_empty() {
        return;
    }
    let per_voice = [
        p.ai_pitch.as_ptr(),
        p.ai_index_enabled.as_ptr(),
        p.ai_index_rate.as_ptr(),
        p.ai_breath.as_ptr(),
        p.ai_protect.as_ptr(),
        p.ai_volume_follow.as_ptr(),
    ];
    if per_voice.contains(&ptr)
        && let Err(e) = vc_core::ai::library::save_voice_settings(std::path::Path::new(&voice), p)
    {
        log::warn!("could not save the voice's settings: {e:#}");
    }
}

#[cfg(not(feature = "ai"))]
fn remember_voice_setting(_host: &dyn Host, _ptr: ParamPtr) {}

// --------------------------------------------------------------- favorites

fn favorites_path() -> Option<std::path::PathBuf> {
    vc_core::presets::config_dir().map(|d| d.join("favorites.txt"))
}

fn load_favorites() -> Vec<String> {
    favorites_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| {
            t.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

fn save_favorites(list: &[String]) {
    if let Some(p) = favorites_path() {
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&p, list.join("\n")) {
            log::warn!("could not save favorites: {e}");
        }
    }
}

/// `Padding` from four sides (iced only has 1- and 2-value array shorthands).
pub const fn pad(top: f32, right: f32, bottom: f32, left: f32) -> iced_core::Padding {
    iced_core::Padding {
        top,
        right,
        bottom,
        left,
    }
}

// ------------------------------------------------------------------ params

fn set_normalized(host: &dyn Host, ptr: ParamPtr, normalized: f32) {
    // SAFETY: every `ParamPtr` the UI holds comes from `host.params()`,
    // which outlives the UI.
    unsafe {
        host.gui()
            .raw_set_parameter_normalized(ptr, normalized.clamp(0.0, 1.0));
    }
}

fn gesture(host: &dyn Host, ptr: ParamPtr, normalized: f32) {
    unsafe {
        host.gui().raw_begin_set_parameter(ptr);
        host.gui()
            .raw_set_parameter_normalized(ptr, normalized.clamp(0.0, 1.0));
        host.gui().raw_end_set_parameter(ptr);
    }
}

pub fn set_bool(host: &dyn Host, param: &BoolParam, value: bool) {
    gesture(host, param.as_ptr(), param.preview_normalized(value));
}

/// Store the current settings as the user preset `name`.
fn store_preset(model: &mut Model, host: &dyn Host, name: &str, replaced: bool) {
    let preset = Preset::capture(host.params(), name);
    match vc_core::presets::save_preset(&preset) {
        Ok(_) => {
            model.current_preset = Some(name.to_string());
            model.last_user_preset = Some(name.to_string());
            model.saving = false;
            model.refresh();
            model.toast(
                if replaced {
                    format!("Updated “{name}”")
                } else {
                    format!("Saved “{name}”")
                },
                false,
            );
        }
        Err(e) => model.toast(format!("Could not save: {e}"), true),
    }
}

fn import_preset_file(model: &mut Model, path: &std::path::Path) {
    match vc_core::presets::import_preset(path) {
        Ok(name) => {
            model.refresh();
            model.page = Page::Home;
            model.toast(format!("Added the preset “{name}”"), false);
        }
        Err(e) => model.toast(format!("Could not add that preset: {e}"), true),
    }
}

#[cfg(feature = "ai")]
fn import_voice_file(model: &mut Model, path: &std::path::Path) {
    if model.import.is_some() {
        model.toast(
            "Another voice is still being added. Try again when it is done.",
            true,
        );
        return;
    }
    model.page = Page::Voices;
    model.import = Some(vc_core::ai::library::start_import(
        path.to_string_lossy().to_string(),
        None,
    ));
}

#[cfg(not(feature = "ai"))]
fn import_voice_file(model: &mut Model, _path: &std::path::Path) {
    model.toast("This build has no AI voices.", true);
}

pub fn rename_id() -> iced_core::widget::Id {
    iced_core::widget::Id::new("preset-rename")
}

// ------------------------------------------------------------------ update

pub fn update(model: &mut Model, message: Message, host: &dyn Host) -> Task<Message> {
    match message {
        Message::Tick => {
            let now = Instant::now();
            let dt = now.duration_since(model.last_tick).as_secs_f32().min(0.2);
            model.last_tick = now;
            model.phase = (model.phase + dt * 0.7) % 1.0;
            model.bench = host.bench();
            if now.duration_since(model.last_refresh).as_secs_f32() > 2.0 {
                model.refresh_cache(host);
            } else if now.duration_since(model.last_status).as_secs_f32() > 0.5 {
                model.refresh_status(host);
            }
            let (i, o, gate) = host.meters().read();
            model.meters.push(i, o, gate, dt);
            // Good news goes away by itself; an error stays until it is read.
            if let Some((_, error, at)) = model.toast
                && !error
                && at.elapsed().as_secs_f32() > 4.0
            {
                model.toast = None;
            }
            #[cfg(feature = "ai")]
            if let Some(job) = model.import.clone() {
                match job.stage() {
                    vc_core::ai::Stage::Done(name) => {
                        model.refresh();
                        if let Some(v) = model.voices.iter().find(|v| {
                            v.path
                                .file_stem()
                                .is_some_and(|s| s.to_string_lossy() == name)
                        }) {
                            host.params().set_ai_voice(&v.path.to_string_lossy());
                        }
                        model.toast(format!("Installed “{}”", name.replace('_', " ")), false);
                        model.import = None;
                    }
                    vc_core::ai::Stage::Checked(result) => {
                        model.refresh();
                        model.toast(format!("Checked {}", result.replace('_', " ")), false);
                        model.import = None;
                    }
                    vc_core::ai::Stage::Failed(e) => {
                        model.toast(format!("Failed: {e}"), true);
                        model.import = None;
                    }
                    vc_core::ai::Stage::Idle => model.import = None,
                    _ => {}
                }
            }
        }
        Message::Go(p) => {
            model.page = p;
            model.refresh_cache(host);
        }
        Message::ParamBegin(ptr) => unsafe { host.gui().raw_begin_set_parameter(ptr) },
        Message::ParamPress(ptr, v) => {
            unsafe { host.gui().raw_begin_set_parameter(ptr) };
            set_normalized(host, ptr, v);
        }
        Message::ParamSet(ptr, v) => set_normalized(host, ptr, v),
        Message::ParamEnd(ptr) => {
            unsafe { host.gui().raw_end_set_parameter(ptr) };
            remember_voice_setting(host, ptr);
            model.preset_still_applies(host);
        }
        Message::ParamReset(ptr) => {
            let d = unsafe { ptr.default_normalized_value() };
            gesture(host, ptr, d);
            remember_voice_setting(host, ptr);
            model.preset_still_applies(host);
        }
        Message::ParamBool(ptr, b) => {
            gesture(host, ptr, if b { 1.0 } else { 0.0 });
            remember_voice_setting(host, ptr);
            model.preset_still_applies(host);
        }
        Message::ParamJump(ptr, v) => {
            gesture(host, ptr, v);
            remember_voice_setting(host, ptr);
            model.preset_still_applies(host);
        }
        Message::ToggleFavorite(name) => {
            if let Some(i) = model.favorites.iter().position(|f| *f == name) {
                model.favorites.remove(i);
            } else {
                model.favorites.push(name);
            }
            save_favorites(&model.favorites);
        }
        Message::TogglePower => {
            let p = &host.params().bypass;
            set_bool(host, p, !p.value());
        }
        Message::ToggleMute => {
            let p = &host.params().mute;
            set_bool(host, p, !p.value());
        }
        Message::ToggleMonitor => {
            if let Some(on) = host.monitor() {
                host.set_monitor(!on);
            }
        }
        Message::ApplyPreset(name) => {
            if let Some(entry) = model.presets.iter().find(|e| e.preset.name == name) {
                let preset = entry.preset.clone();
                preset.apply(host.params(), |ptr, n| gesture(host, ptr, n));
                model.last_user_preset = (!entry.is_factory()).then(|| name.clone());
                model.current_preset = Some(name);
            }
        }
        Message::SearchChanged(s) => model.search = s,
        Message::SaveNameChanged(s) => model.save_name = s,
        Message::StartSave => {
            model.saving = true;
            model.save_name.clear();
        }
        Message::CancelSave => model.saving = false,
        Message::SavePreset => {
            let name = model.save_name.trim().to_string();
            if !name.is_empty() {
                let replaced = model
                    .presets
                    .iter()
                    .any(|e| !e.is_factory() && e.preset.name.eq_ignore_ascii_case(&name));
                store_preset(model, host, &name, replaced);
            }
        }
        Message::UpdatePreset(name) => store_preset(model, host, &name, true),
        Message::TogglePresetEdit => {
            model.editing_presets = !model.editing_presets;
            model.confirm_delete_preset = None;
            model.renaming = None;
        }
        Message::AskDeletePreset(name) => {
            model.renaming = None;
            model.confirm_delete_preset = Some(name);
        }
        Message::CancelDeletePreset => model.confirm_delete_preset = None,
        Message::StartRenamePreset(name) => {
            model.confirm_delete_preset = None;
            model.renaming = Some((name.clone(), name));
            return iced_runtime::widget::operation::focus(rename_id());
        }
        Message::RenameChanged(s) => {
            if let Some((_, new)) = model.renaming.as_mut() {
                *new = s;
            }
        }
        Message::CommitRename => {
            if let Some((old, new)) = model.renaming.take() {
                let new = new.trim().to_string();
                let path = model
                    .presets
                    .iter()
                    .find(|e| e.preset.name == old)
                    .and_then(|e| e.path.clone());
                if let Some(path) = path
                    && !new.is_empty()
                    && new != old
                {
                    match vc_core::presets::rename_preset(&path, &new) {
                        Ok(_) => {
                            for slot in [&mut model.current_preset, &mut model.last_user_preset] {
                                if slot.as_deref() == Some(old.as_str()) {
                                    *slot = Some(new.clone());
                                }
                            }
                            if let Some(f) = model.favorites.iter_mut().find(|f| **f == old) {
                                *f = new.clone();
                                save_favorites(&model.favorites);
                            }
                            model.refresh();
                        }
                        Err(e) => model.toast(format!("Could not rename: {e}"), true),
                    }
                }
            }
        }
        Message::PickFile(_) => {
            // Answered by the host (it owns the file dialogs).
        }
        Message::FilePicked(request, path) => {
            if let Some(path) = path {
                match request {
                    FileRequest::ImportPreset => import_preset_file(model, &path),
                    FileRequest::ExportPreset(name) => {
                        let found = model
                            .presets
                            .iter()
                            .find(|e| e.preset.name == name)
                            .map(|e| e.preset.clone());
                        if let Some(preset) = found {
                            match vc_core::presets::export_preset(&preset, &path) {
                                Ok(()) => model
                                    .toast(format!("Saved “{name}” to {}", path.display()), false),
                                Err(e) => {
                                    model.toast(format!("Could not save the file: {e}"), true)
                                }
                            }
                        }
                    }
                    FileRequest::ImportVoice => import_voice_file(model, &path),
                }
            }
        }
        Message::FileDropped(path) => {
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            match ext.as_str() {
                "ron" => import_preset_file(model, &path),
                "onnx" | "pth" | "zip" => import_voice_file(model, &path),
                _ => model.toast(
                    "That file is neither a preset (.ron) nor a voice (.onnx, .pth, .zip).",
                    true,
                ),
            }
        }
        Message::DeletePreset(name) => {
            model.confirm_delete_preset = None;
            if let Some(path) = model
                .presets
                .iter()
                .find(|e| e.preset.name == name)
                .and_then(|e| e.path.clone())
            {
                match vc_core::presets::delete_preset(&path) {
                    Ok(()) => {
                        for slot in [&mut model.current_preset, &mut model.last_user_preset] {
                            if slot.as_deref() == Some(name.as_str()) {
                                *slot = None;
                            }
                        }
                        model.refresh();
                        model.toast(format!("Deleted “{name}”"), false);
                    }
                    Err(e) => model.toast(format!("Could not delete: {e}"), true),
                }
            }
        }
        Message::RefreshLibrary => {
            model.refresh();
            model.refresh_cache(host);
        }
        Message::RouteApp(id, on) => {
            host.set_app_route(id, on);
            if let Some(s) = model.cache.app_streams.iter_mut().find(|s| s.id == id) {
                s.routed = on;
            }
        }
        Message::SetDefaultSource(on) => {
            host.set_default_source(on);
            model.cache.default_source = Some(on);
        }
        Message::RefreshApps => model.refresh_cache(host),
        Message::SelectMic(t) => {
            host.set_capture_target(t.clone());
            model.cache.capture_target = t;
        }
        Message::SelectOutput(t) => {
            host.set_output_target(t.clone());
            model.cache.output_target = t;
        }
        Message::ConfigureHotkey => host.configure_hotkey(),
        Message::Setting(s) => {
            host.apply_setting(s);
            model.cache.settings = host.settings();
            model.cache.hardware = host.hardware();
        }
        Message::RetryAudio => {
            host.retry_audio();
            model.refresh_cache(host);
        }
        Message::DismissWelcome => {
            host.apply_setting(HostSetting::DismissWelcome);
            model.cache.settings = host.settings();
        }
        Message::ToggleAiAdvanced => model.ai_advanced = !model.ai_advanced,
        Message::OpenDataFolder => host.open_data_folder(),
        Message::Quit => host.quit(),
        Message::DismissToast => model.toast = None,
        Message::ImportUrlChanged(s) => model.import_url = s,
        Message::ImportNameChanged(s) => model.import_name = s,
        #[cfg(feature = "ai")]
        Message::StartImport => {
            let url = model.import_url.trim().to_string();
            if !url.is_empty() && model.import.is_none() {
                let name = if model.import_name.trim().is_empty() {
                    None
                } else {
                    Some(model.import_name.trim().to_string())
                };
                model.import = Some(vc_core::ai::library::start_import(url, name));
                model.import_url.clear();
                model.import_name.clear();
            }
        }
        #[cfg(feature = "ai")]
        Message::CancelImport => {
            if let Some(j) = &model.import {
                j.cancel();
            }
        }
        #[cfg(feature = "ai")]
        Message::DismissImport => model.import = None,
        #[cfg(feature = "ai")]
        Message::UseVoice(path) => {
            select_voice(host, &path);
            set_bool(host, &host.params().ai_enabled, true);
        }
        #[cfg(feature = "ai")]
        Message::SelectVoice(path) => select_voice(host, &path),
        #[cfg(feature = "ai")]
        Message::SetVoiceSpeakers(path, n) => {
            if let Err(e) = vc_core::ai::library::set_voice_speakers(std::path::Path::new(&path), n)
            {
                model.toast(format!("Could not save: {e}"), true);
            }
            model.refresh();
        }
        #[cfg(feature = "ai")]
        Message::SetDefaultSpeaker(path, idx) => {
            if let Err(e) =
                vc_core::ai::library::set_voice_default_speaker(std::path::Path::new(&path), idx)
            {
                model.toast(format!("Could not save: {e}"), true);
            }
            if host.params().ai_voice() == path {
                let p = &host.params().ai_speaker;
                gesture(host, p.as_ptr(), p.preview_normalized(idx as i32));
            }
            model.refresh();
        }
        Message::SpeakerNameChanged(path, idx, name) => {
            model.speaker_edits.insert((path, idx), name);
        }
        #[cfg(feature = "ai")]
        Message::SaveSpeakerName(path, idx) => {
            if let Some(name) = model.speaker_edits.remove(&(path.clone(), idx)) {
                if let Err(e) = vc_core::ai::library::set_voice_speaker_name(
                    std::path::Path::new(&path),
                    idx,
                    &name,
                ) {
                    model.toast(format!("Could not save: {e}"), true);
                }
                model.refresh();
            }
        }
        Message::ResetMany(ptrs) => {
            for ptr in ptrs {
                let d = unsafe { ptr.default_normalized_value() };
                gesture(host, ptr, d);
            }
            model.preset_still_applies(host);
        }
        Message::Escape => {
            model.saving = false;
            model.renaming = None;
            model.confirm_delete_preset = None;
            model.search.clear();
            model.confirm_delete = None;
            model.speaker_edits.clear();
        }
        Message::FocusSearch => {
            model.page = Page::Home;
            return iced_runtime::widget::operation::focus(search_id());
        }
        Message::AskDeleteVoice(path) => model.confirm_delete = Some(path),
        Message::CancelDeleteVoice => model.confirm_delete = None,
        #[cfg(feature = "ai")]
        Message::ConfirmDeleteVoice => {
            if let Some(path) = model.confirm_delete.take() {
                if host.params().ai_voice() == path {
                    host.params().set_ai_voice("");
                    set_bool(host, &host.params().ai_enabled, false);
                }
                if let Err(e) = vc_core::ai::library::delete_voice(std::path::Path::new(&path)) {
                    model.toast(format!("Could not delete: {e}"), true);
                }
                model.refresh();
            }
        }
        #[cfg(feature = "ai")]
        Message::SelectDevice(d) => {
            // Picking the graphics card again is the way to retry after it
            // was set aside.
            if d.starts_with("dml:") {
                vc_core::ai::compute::directml_forget_crash();
            }
            host.params().set_ai_device(&d);
        }
        #[cfg(feature = "ai")]
        Message::DownloadBaseModels => {
            if model.import.is_none() {
                model.import = Some(vc_core::ai::library::start_base_models_download());
            }
        }
        Message::BenchRecord(name) => host.bench_record(&name),
        Message::BenchPlay(name) => host.bench_play(name.as_deref()),
        #[cfg(feature = "ai")]
        Message::CheckVoice(path) => {
            if model.import.is_none() {
                model.import = Some(vc_core::ai::library::start_voice_check(path.into()));
            }
        }
        #[cfg(not(feature = "ai"))]
        Message::CheckVoice(_) => {}
        #[cfg(not(feature = "ai"))]
        Message::StartImport
        | Message::CancelImport
        | Message::DismissImport
        | Message::UseVoice(_)
        | Message::SelectVoice(_)
        | Message::ConfirmDeleteVoice
        | Message::SelectDevice(_)
        | Message::SetDefaultSpeaker(..)
        | Message::SetVoiceSpeakers(..)
        | Message::SaveSpeakerName(..)
        | Message::DownloadBaseModels => {}
    }
    Task::none()
}

// -------------------------------------------------------------------- view

pub fn view<'a>(model: &'a Model, host: &'a dyn Host) -> Element<'a> {
    let body = match model.page {
        Page::Home => pages::home::view(model, host),
        Page::Voices => pages::voices::view(model, host),
        Page::Effects => pages::effects::view(model, host),
        Page::Mixer => pages::mixer::view(model, host),
        Page::Settings => pages::settings::view(model, host),
    };
    let content = column![
        pages::topbar::view(model, host),
        scrollable(
            container(body)
                .padding(pad(8.0, 24.0, 24.0, 24.0))
                .width(Length::Fill)
        )
        .height(Length::Fill),
    ]
    .width(Length::Fill);

    let main = row![
        sidebar(model, host),
        container(content).width(Length::Fill).height(Length::Fill)
    ]
    .width(Length::Fill)
    .height(Length::Fill);

    let mut layers = iced_widget::stack![main];
    if let Some((msg, error, _)) = &model.toast {
        let (color, ink) = if *error {
            (theme::DANGER, iced_core::Color::WHITE)
        } else {
            (theme::ACCENT, theme::ON_ACCENT)
        };
        let toast = container(
            row![
                if *error {
                    widgets::icons::alert(16.0, ink)
                } else {
                    widgets::icons::check(16.0, ink)
                },
                text(msg.as_str())
                    .size(14)
                    .wrapping(iced_core::text::Wrapping::WordOrGlyph),
                button(widgets::icons::close(14.0, ink))
                    .style(theme::button_ghost)
                    .padding(4)
                    .on_press(Message::DismissToast)
            ]
            .spacing(10)
            .align_y(iced_core::Alignment::Center),
        )
        .padding([10, 14])
        .max_width(520)
        .style(move |_theme| container::Style {
            background: Some(color.into()),
            text_color: Some(ink),
            border: theme::radius(12.0),
            ..container::Style::default()
        });
        layers = layers.push(
            container(toast)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(iced_core::alignment::Horizontal::Right)
                .align_y(iced_core::alignment::Vertical::Bottom)
                .padding(20),
        );
    }
    layers.into()
}

fn sidebar<'a>(model: &'a Model, host: &'a dyn Host) -> Element<'a> {
    let t = theme::tokens(host.theme_mode());
    let mut col = column![
        container(widgets::icons::logo(44.0))
            .padding(pad(6.0, 0.0, 14.0, 0.0))
            .center_x(Length::Fill)
    ]
    .spacing(4)
    .padding(10)
    .width(Length::Fixed(176.0));
    for page in Page::ALL {
        if page == Page::Voices && !cfg!(feature = "ai") {
            continue;
        }
        let active = model.page == page;
        let color = if active { page.accent() } else { t.text_dim };
        let icon = match page {
            Page::Home => widgets::icons::home(18.0, color),
            Page::Voices => widgets::icons::mask(18.0, color),
            Page::Effects => widgets::icons::sliders(18.0, color),
            Page::Mixer => widgets::icons::mixer(18.0, color),
            Page::Settings => widgets::icons::gear(18.0, color),
        };
        col = col.push(
            button(
                row![icon, text(page.title()).size(15)]
                    .spacing(12)
                    .align_y(iced_core::Alignment::Center),
            )
            .width(Length::Fill)
            .padding([10, 12])
            .style(theme::nav_button(active, page.accent()))
            .on_press(Message::Go(page)),
        );
    }
    col = col.push(iced_widget::space::vertical());
    col = col.push(pages::topbar::status_pill(model, host));
    container(col)
        .height(Length::Fill)
        .style(theme::sidebar)
        .into()
}

/// Hover explanation: appears after a short delay and follows the cursor.
/// An empty text returns the content unchanged.
pub fn tip<'a>(content: impl Into<Element<'a>>, text_: &'a str) -> Element<'a> {
    if text_.is_empty() {
        return content.into();
    }
    tooltip(
        content,
        container(
            text(text_)
                .size(12.5)
                .wrapping(iced_core::text::Wrapping::WordOrGlyph),
        )
        .padding([8, 10])
        .max_width(320)
        .style(theme::help_panel),
        tooltip::Position::Top,
    )
    .delay(std::time::Duration::from_millis(650))
    .gap(8)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nice_plug::context::gui::GuiContext;
    use std::sync::Arc;

    /// The least a host can be: parameters and nothing else.
    struct NullGui;

    impl GuiContextInner for NullGui {
        fn plugin_api(&self) -> PluginApi {
            PluginApi::Standalone
        }
        unsafe fn raw_begin_set_parameter(&self, _param: ParamPtr) {}
        unsafe fn raw_set_parameter_normalized(&self, param: ParamPtr, normalized: f32) {
            // SAFETY: the pointer comes from `TestHost::params`, which
            // outlives every call made in a test.
            unsafe {
                param._internal_set_normalized_value(normalized);
            }
        }
        unsafe fn raw_end_set_parameter(&self, _param: ParamPtr) {}
        fn get_state(&self) -> PluginState {
            PluginState {
                version: String::new(),
                params: Default::default(),
                fields: Default::default(),
            }
        }
        fn set_state(&self, _state: PluginState) {}
        fn request_restart(&self) {}
    }

    struct TestHost {
        params: Arc<vc_core::VcParams>,
        gui: GuiContext,
        meters: vc_core::Meters,
    }

    impl TestHost {
        fn new() -> Self {
            Self {
                params: Arc::new(vc_core::VcParams::default()),
                gui: GuiContext::new(Arc::new(NullGui)),
                meters: vc_core::Meters::default(),
            }
        }
    }

    impl Host for TestHost {
        fn params(&self) -> &vc_core::VcParams {
            &self.params
        }
        fn gui(&self) -> &GuiContext {
            &self.gui
        }
        fn meters(&self) -> &vc_core::Meters {
            &self.meters
        }
        fn theme_mode(&self) -> Mode {
            Mode::Dark
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn latency_ms(&self) -> f32 {
            0.0
        }
    }

    /// Presets are files: keep the tests out of the real settings folder.
    #[cfg(target_os = "linux")]
    fn private_settings_folder() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let dir = std::env::temp_dir().join(format!("vc-gui-tests-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            // SAFETY: set once, before any test in this module reads it.
            unsafe { std::env::set_var("XDG_CONFIG_HOME", &dir) };
        });
    }

    fn send(model: &mut Model, host: &TestHost, m: Message) {
        let _ = update(model, m, host);
    }

    #[test]
    fn mute_is_a_switch_of_its_own() {
        let host = TestHost::new();
        let mut model = Model::new();
        send(&mut model, &host, Message::ToggleMute);
        assert!(host.params.mute.value());
        assert!(!host.params.bypass.value(), "muting does not switch off");
        send(&mut model, &host, Message::ToggleMute);
        assert!(!host.params.mute.value());
    }

    #[test]
    fn the_preset_name_goes_once_a_control_is_changed() {
        let host = TestHost::new();
        let mut model = Model::new();
        let name = model
            .presets
            .iter()
            .find(|e| !e.preset.matches(&host.params))
            .map(|e| e.preset.name.clone())
            .expect("a preset that changes something");
        send(&mut model, &host, Message::ApplyPreset(name.clone()));
        assert_eq!(model.current_preset.as_deref(), Some(name.as_str()));
        // Muting and switching off are not part of a voice: the name stays.
        send(&mut model, &host, Message::ToggleMute);
        send(&mut model, &host, Message::TogglePower);
        model.preset_still_applies(&host);
        assert_eq!(model.current_preset.as_deref(), Some(name.as_str()));
        // Turning a knob is.
        let pitch = host.params.pitch.as_ptr();
        let moved = (host.params.pitch.unmodulated_normalized_value() + 0.2).min(1.0);
        send(&mut model, &host, Message::ParamJump(pitch, moved));
        assert_eq!(model.current_preset, None);
    }

    #[test]
    fn errors_stay_until_dismissed_and_good_news_leaves() {
        let host = TestHost::new();
        let mut model = Model::new();
        let old = Instant::now() - std::time::Duration::from_secs(30);
        model.toast = Some(("saved".into(), false, old));
        send(&mut model, &host, Message::Tick);
        assert!(model.toast.is_none());
        model.toast = Some(("that failed".into(), true, old));
        send(&mut model, &host, Message::Tick);
        assert!(model.toast.is_some());
        send(&mut model, &host, Message::DismissToast);
        assert!(model.toast.is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_preset_can_be_saved_renamed_updated_and_deleted() {
        private_settings_folder();
        let host = TestHost::new();
        let mut model = Model::new();
        let has = |model: &Model, name: &str| model.presets.iter().any(|e| e.preset.name == name);

        // A built-in name is refused, with an error the user can read.
        let factory = model.presets[0].preset.name.clone();
        model.save_name = factory.clone();
        send(&mut model, &host, Message::SavePreset);
        assert!(model.toast.as_ref().is_some_and(|t| t.1), "an error toast");
        assert_eq!(
            model
                .presets
                .iter()
                .filter(|e| e.preset.name == factory)
                .count(),
            1
        );

        model.save_name = "Stage voice".into();
        send(&mut model, &host, Message::SavePreset);
        assert!(has(&model, "Stage voice"));
        assert_eq!(model.current_preset.as_deref(), Some("Stage voice"));

        // Change something: the name goes, and Update puts it back.
        let pitch = host.params.pitch.as_ptr();
        send(&mut model, &host, Message::ParamJump(pitch, 0.9));
        assert_eq!(model.current_preset, None);
        assert_eq!(model.last_user_preset.as_deref(), Some("Stage voice"));
        send(
            &mut model,
            &host,
            Message::UpdatePreset("Stage voice".into()),
        );
        assert_eq!(model.current_preset.as_deref(), Some("Stage voice"));
        let stored = model
            .presets
            .iter()
            .find(|e| e.preset.name == "Stage voice")
            .unwrap();
        assert!(stored.preset.matches(&host.params));

        send(
            &mut model,
            &host,
            Message::StartRenamePreset("Stage voice".into()),
        );
        send(
            &mut model,
            &host,
            Message::RenameChanged("Radio show".into()),
        );
        send(&mut model, &host, Message::CommitRename);
        assert!(has(&model, "Radio show") && !has(&model, "Stage voice"));
        assert_eq!(model.current_preset.as_deref(), Some("Radio show"));

        // Deleting asks first; Esc backs out.
        send(
            &mut model,
            &host,
            Message::AskDeletePreset("Radio show".into()),
        );
        send(&mut model, &host, Message::Escape);
        assert!(has(&model, "Radio show"));
        send(
            &mut model,
            &host,
            Message::AskDeletePreset("Radio show".into()),
        );
        send(
            &mut model,
            &host,
            Message::DeletePreset("Radio show".into()),
        );
        assert!(!has(&model, "Radio show"));
        assert_eq!(model.current_preset, None);
    }
}
