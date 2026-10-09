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

pub use host::{AppStream, Capabilities, Device, Host, HostSetting, HostStats};
pub use iced_runtime::Task;
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
    ToggleMonitor,
    ApplyPreset(String),
    SearchChanged(String),
    SaveNameChanged(String),
    StartSave,
    CancelSave,
    SavePreset,
    DeletePreset(String),
    RefreshLibrary,
    SelectMic(Option<String>),
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
    pub toast: Option<(String, bool, Instant)>,
    pub meters: widgets::meter::Levels,
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
}

/// Values that are expensive or non-trivial to compute, snapshotted for `view`.
#[derive(Default, Clone)]
pub struct Cache {
    pub devices: Vec<Device>,
    pub capture_target: Option<String>,
    pub app_streams: Vec<AppStream>,
    pub default_source: Option<bool>,
    pub base_models_present: bool,
    pub voices_dir: String,
    /// (setting key, label) for the compute picker.
    pub compute_options: Vec<(String, String)>,
    pub gpu_note: Option<String>,
    pub runtime: Option<String>,
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
            toast: None,
            meters: widgets::meter::Levels::default(),
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
        }
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
        self.cache.app_streams = host.app_streams();
        self.cache.default_source = host.default_source();
        self.cache.runtime = host.runtime_description();
        #[cfg(feature = "ai")]
        {
            use vc_core::ai::compute;
            self.cache.base_models_present = vc_core::ai::library::base_models_present();
            self.cache.voices_dir = vc_core::ai::voices_dir()
                .map(|d| d.display().to_string())
                .unwrap_or_default();
            let gpus = compute::gpus();
            let mut opts = vec![
                (
                    "auto".to_string(),
                    match compute::resolve("auto") {
                        compute::Backend::Cpu => "Auto (CPU)".to_string(),
                        compute::Backend::Rocm { device } | compute::Backend::Cuda { device } => {
                            format!("Auto (GPU {device})")
                        }
                    },
                ),
                ("cpu".to_string(), "CPU".to_string()),
            ];
            for g in &gpus {
                let backend = if g.api == "CUDA" {
                    compute::Backend::Cuda { device: g.index }
                } else {
                    compute::Backend::Rocm { device: g.index }
                };
                opts.push((
                    compute::setting_string(Some(backend)),
                    format!("GPU {} · {} ({})", g.index, g.name, g.api),
                ));
            }
            self.cache.compute_options = opts;
            self.cache.gpu_note = compute::gpu_note();
        }
    }

    pub fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toast = Some((text.into(), error, Instant::now()));
    }
}

/// Select a voice and start it on its pinned speaker.
#[cfg(feature = "ai")]
fn select_voice(host: &dyn Host, path: &str) {
    host.params().set_ai_voice(path);
    if !path.is_empty() {
        let idx = vc_core::ai::library::default_speaker_for(std::path::Path::new(path));
        let p = &host.params().ai_speaker;
        gesture(host, p.as_ptr(), p.preview_normalized(idx));
    }
}

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

// ------------------------------------------------------------------ update

pub fn update(model: &mut Model, message: Message, host: &dyn Host) -> Task<Message> {
    match message {
        Message::Tick => {
            let now = Instant::now();
            let dt = now.duration_since(model.last_tick).as_secs_f32().min(0.2);
            model.last_tick = now;
            if now.duration_since(model.last_refresh).as_secs_f32() > 2.0 {
                model.refresh_cache(host);
            }
            let (i, o, gate) = host.meters().read();
            model.meters.push(i, o, gate, dt);
            if let Some((_, _, at)) = model.toast
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
                    vc_core::ai::Stage::Failed(e) => {
                        model.toast(format!("Import failed: {e}"), true);
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
        Message::ParamEnd(ptr) => unsafe { host.gui().raw_end_set_parameter(ptr) },
        Message::ParamReset(ptr) => {
            let d = unsafe { ptr.default_normalized_value() };
            gesture(host, ptr, d);
        }
        Message::ParamBool(ptr, b) => gesture(host, ptr, if b { 1.0 } else { 0.0 }),
        Message::ParamJump(ptr, v) => gesture(host, ptr, v),
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
        Message::ToggleMonitor => {
            if let Some(on) = host.monitor() {
                host.set_monitor(!on);
            }
        }
        Message::ApplyPreset(name) => {
            if let Some(entry) = model.presets.iter().find(|e| e.preset.name == name) {
                let preset = entry.preset.clone();
                preset.apply(host.params(), |ptr, n| gesture(host, ptr, n));
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
                let preset = Preset::capture(host.params(), &name);
                match vc_core::presets::save_preset(&preset) {
                    Ok(_) => {
                        model.current_preset = Some(name.clone());
                        model.saving = false;
                        model.refresh();
                        model.toast(format!("Saved “{name}”"), false);
                    }
                    Err(e) => model.toast(format!("Could not save: {e}"), true),
                }
            }
        }
        Message::DeletePreset(name) => {
            if let Some(path) = model
                .presets
                .iter()
                .find(|e| e.preset.name == name)
                .and_then(|e| e.path.clone())
            {
                match vc_core::presets::delete_preset(&path) {
                    Ok(()) => {
                        if model.current_preset.as_deref() == Some(name.as_str()) {
                            model.current_preset = None;
                        }
                        model.refresh();
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
        Message::ConfigureHotkey => host.configure_hotkey(),
        Message::Setting(s) => host.apply_setting(s),
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
        }
        Message::Escape => {
            model.saving = false;
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
        Message::SelectDevice(d) => host.params().set_ai_device(&d),
        #[cfg(feature = "ai")]
        Message::DownloadBaseModels => {
            if model.import.is_none() {
                model.import = Some(vc_core::ai::library::start_base_models_download());
            }
        }
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
        let color = if *error { theme::DANGER } else { theme::ACCENT };
        let toast = container(
            row![
                text(msg.as_str()).size(14),
                button(widgets::icons::close(14.0, iced_core::Color::WHITE))
                    .style(theme::button_ghost)
                    .padding(4)
                    .on_press(Message::DismissToast)
            ]
            .spacing(10)
            .align_y(iced_core::Alignment::Center),
        )
        .padding([10, 14])
        .style(move |_theme| container::Style {
            background: Some(color.into()),
            text_color: Some(iced_core::Color::WHITE),
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
