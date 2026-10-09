//! The desktop window: an iced daemon around the shared `vc-gui` surface.
//! A daemon (not an application) because on Wayland a window cannot be
//! hidden, only closed; "close to tray" closes it and the tray reopens it.

use iced::{Element, Subscription, Task, Theme, keyboard, window};
use nice_plug::context::gui::GuiContext;
use std::cell::Cell;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use vc_core::{Meters, VcParams};
use vc_gui::host::{Device, HostSettings};
use vc_gui::{Capabilities, Host, HostSetting, HostStats, Mode, Model, Page};

use crate::config::{self, AppConfig};
use crate::control::Shared;
use crate::gui_ctx::StandaloneGuiContext;

pub const APP_ID: &str = "io.github.zerodaylabz.VoiceChanger";
const ICON_PNG: &[u8] = include_bytes!("../../../packaging/icons/128.png");

#[derive(Clone)]
pub struct Options {
    pub start_hidden: bool,
    /// Render one page to a PNG and quit (documentation screenshots).
    pub screenshot: Option<(PathBuf, Page)>,
}

// ------------------------------------------------------------------- host

pub struct AppHost {
    shared: Arc<Shared>,
    gui: GuiContext,
    theme: Cell<Mode>,
}

impl AppHost {
    fn new(shared: Arc<Shared>) -> Self {
        let theme = shared
            .config
            .lock()
            .map(|c| mode_from(&c.theme))
            .unwrap_or_default();
        Self {
            gui: GuiContext::new(Arc::new(StandaloneGuiContext {
                shared: shared.clone(),
            })),
            shared,
            theme: Cell::new(theme),
        }
    }

    fn with_config(&self, f: impl FnOnce(&mut AppConfig)) {
        if let Ok(mut c) = self.shared.config.lock() {
            f(&mut c);
            if let Err(e) = c.save() {
                log::warn!("could not save config: {e}");
            }
        }
    }
}

pub fn mode_from(s: &str) -> Mode {
    if s.eq_ignore_ascii_case("light") {
        Mode::Light
    } else {
        Mode::Dark
    }
}

impl Host for AppHost {
    fn params(&self) -> &VcParams {
        &self.shared.params
    }
    fn gui(&self) -> &GuiContext {
        &self.gui
    }
    fn meters(&self) -> &Meters {
        &self.shared.meters
    }
    fn theme_mode(&self) -> Mode {
        self.theme.get()
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            standalone: true,
            monitor: true,
            microphone: true,
            hotkey: true,
            settings: true,
        }
    }
    fn latency_ms(&self) -> f32 {
        let mode: vc_dsp::PitchMode = self.shared.params.pitch_engine.value().into();
        let mut samples = mode.latency_samples(self.shared.sample_rate) as f32;
        samples += self.shared.quantum as f32; // one block of output buffering
        samples / self.shared.sample_rate * 1000.0
    }
    #[cfg(feature = "ai")]
    fn ai_status(&self) -> Option<Arc<vc_core::ai::AiStatus>> {
        self.shared
            .audio
            .lock()
            .ok()
            .and_then(|a| a.as_ref().map(|a| a.ai_status.clone()))
    }
    fn stats(&self) -> Option<HostStats> {
        Some(HostStats {
            underruns: self.shared.stats.underruns.load(Ordering::Relaxed),
            process_max_us: self.shared.stats.process_max_us.load(Ordering::Relaxed),
            quantum_us: (self.shared.quantum as f32 / self.shared.sample_rate * 1_000_000.0) as u64,
        })
    }
    fn devices(&self) -> Vec<Device> {
        self.shared
            .devices()
            .into_iter()
            .map(|d| Device {
                name: d.name,
                description: d.description,
            })
            .collect()
    }
    fn capture_target(&self) -> Option<String> {
        self.shared
            .audio
            .lock()
            .ok()
            .and_then(|a| a.as_ref().and_then(|a| a.capture_target()))
    }
    fn set_capture_target(&self, target: Option<String>) {
        self.shared.set_capture_target(target);
    }
    fn monitor(&self) -> Option<bool> {
        Some(self.shared.monitor())
    }
    fn set_monitor(&self, on: bool) {
        self.shared.set_monitor(on);
    }
    fn app_streams(&self) -> Vec<vc_gui::AppStream> {
        self.shared
            .app_streams()
            .into_iter()
            .map(|s| vc_gui::AppStream {
                id: s.id,
                app: s.app,
                media: s.media,
                routed: s.routed,
            })
            .collect()
    }
    fn set_app_route(&self, stream_id: u32, on: bool) {
        self.shared.route_app(stream_id, on);
    }
    fn default_source(&self) -> Option<bool> {
        Some(self.shared.is_default_source())
    }
    fn set_default_source(&self, on: bool) {
        self.shared.set_default_source(on);
        self.with_config(|_| {});
    }
    #[cfg(feature = "ai")]
    fn runtime_description(&self) -> Option<String> {
        Some(vc_core::ai::compute::runtime_description())
    }
    fn hotkey(&self) -> Option<Option<String>> {
        Some(
            self.shared
                .hotkey_trigger
                .lock()
                .ok()
                .and_then(|t| t.clone()),
        )
    }
    fn configure_hotkey(&self) {
        self.shared.configure_hotkey.store(true, Ordering::SeqCst);
    }
    fn settings(&self) -> HostSettings {
        let c = self
            .shared
            .config
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default();
        HostSettings {
            close_to_tray: c.close_to_tray,
            start_minimized: c.start_minimized,
            autostart: Some(config::autostart_enabled()),
        }
    }
    fn apply_setting(&self, setting: HostSetting) {
        match setting {
            HostSetting::CloseToTray(b) => self.with_config(|c| c.close_to_tray = b),
            HostSetting::StartMinimized(b) => self.with_config(|c| c.start_minimized = b),
            HostSetting::Autostart(b) => {
                if let Err(e) = config::set_autostart(b) {
                    log::warn!("autostart: {e}");
                }
            }
            HostSetting::Theme(m) => {
                self.theme.set(m);
                self.with_config(|c| {
                    c.theme = if m == Mode::Light {
                        "light".into()
                    } else {
                        "dark".into()
                    }
                });
            }
        }
    }
    fn log_lines(&self) -> Vec<String> {
        crate::logger::lines()
    }
    fn open_data_folder(&self) {
        if let Some(dir) = directories::ProjectDirs::from("", "echo", "voice-changer")
            .map(|d| d.data_dir().to_path_buf())
        {
            let _ = std::fs::create_dir_all(&dir);
            if let Err(e) = std::process::Command::new("xdg-open").arg(&dir).spawn() {
                log::warn!("could not open {}: {e}", dir.display());
            }
        }
    }
    fn quit(&self) {
        self.shared.request_quit();
    }
}

// -------------------------------------------------------------------- app

pub struct App {
    model: Model,
    host: AppHost,
    shared: Arc<Shared>,
    window: Option<window::Id>,
    screenshot: Option<(PathBuf, Page, u32)>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Ui(vc_gui::Message),
    Tick,
    Opened,
    CloseRequested(window::Id),
    Keyboard(keyboard::Event),
    Window(window::Event),
    Shot(window::Screenshot),
}

/// `VC_SCREENSHOT_SIZE=WxH` overrides the 1200×800 documentation size.
fn screenshot_size() -> iced::Size {
    std::env::var("VC_SCREENSHOT_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some(iced::Size::new(w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or(iced::Size::new(1200.0, 820.0))
}

fn window_settings(screenshot: bool, remembered: Option<[f32; 4]>) -> window::Settings {
    let size = match remembered {
        Some(w) if !screenshot && w[2] >= vc_gui::MIN_WIDTH && w[3] >= vc_gui::MIN_HEIGHT => {
            iced::Size::new(w[2], w[3])
        }
        _ if screenshot => screenshot_size(),
        _ => iced::Size::new(vc_gui::DEFAULT_WIDTH, vc_gui::DEFAULT_HEIGHT),
    };
    let position = match remembered {
        Some(w) if !screenshot && w[0].is_finite() && w[1].is_finite() => {
            window::Position::Specific(iced::Point::new(w[0], w[1]))
        }
        _ => window::Position::Default,
    };
    window::Settings {
        size,
        position,
        min_size: Some(iced::Size::new(vc_gui::MIN_WIDTH, vc_gui::MIN_HEIGHT)),
        icon: window::icon::from_file_data(ICON_PNG, None).ok(),
        platform_specific: window::settings::PlatformSpecific {
            application_id: APP_ID.into(),
            ..Default::default()
        },
        exit_on_close_request: false,
        ..window::Settings::default()
    }
}

impl App {
    fn open_window(&mut self) -> Task<Message> {
        if self.window.is_some() {
            return Task::none();
        }
        let remembered = self.shared.config.lock().ok().and_then(|c| c.window);
        let (id, task) = window::open(window_settings(self.screenshot.is_some(), remembered));
        self.window = Some(id);
        task.map(|_| Message::Opened)
    }

    fn sync_from_shared(&mut self) {
        self.model.current_preset = self
            .shared
            .current_preset
            .lock()
            .ok()
            .and_then(|c| c.clone());
    }

    fn sync_to_shared(&self) {
        if let Ok(mut cur) = self.shared.current_preset.lock() {
            *cur = self.model.current_preset.clone();
        }
    }

    fn ui(&mut self, m: vc_gui::Message) -> Task<Message> {
        let task = vc_gui::update(&mut self.model, m, &self.host);
        self.sync_to_shared();
        task.map(Message::Ui)
    }
}

fn update(app: &mut App, message: Message) -> Task<Message> {
    match message {
        Message::Tick => {
            if app.shared.quit.load(Ordering::SeqCst) {
                return iced::exit();
            }
            let mut task = Task::none();
            if app.shared.show_window.swap(false, Ordering::SeqCst) {
                task = match app.window {
                    Some(id) => window::gain_focus(id),
                    None => app.open_window(),
                };
            }
            app.sync_from_shared();
            let t2 = app.ui(vc_gui::Message::Tick);
            if let (Some((_, _, frames)), Some(id)) = (app.screenshot.as_mut(), app.window) {
                *frames += 1;
                if *frames == 45 {
                    return window::screenshot(id).map(Message::Shot);
                }
            }
            Task::batch([task, t2])
        }
        Message::Ui(m) => app.ui(m),
        Message::Opened => Task::none(),
        Message::CloseRequested(id) => {
            let to_tray = app
                .shared
                .config
                .lock()
                .map(|c| c.close_to_tray)
                .unwrap_or(false);
            if !to_tray {
                app.shared.request_quit();
            }
            app.window = None;
            window::close(id)
        }
        Message::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
            if modifiers.command() =>
        {
            use keyboard::key::Named;
            let msg = match key.as_ref() {
                keyboard::Key::Character("m") => Some(vc_gui::Message::TogglePower),
                keyboard::Key::Character("h") => Some(vc_gui::Message::ToggleMonitor),
                keyboard::Key::Character("1") => Some(vc_gui::Message::Go(Page::Home)),
                keyboard::Key::Character("2") => Some(vc_gui::Message::Go(Page::Voices)),
                keyboard::Key::Character("3") => Some(vc_gui::Message::Go(Page::Effects)),
                keyboard::Key::Character("4") => Some(vc_gui::Message::Go(Page::Mixer)),
                keyboard::Key::Character("5") | keyboard::Key::Character(",") => {
                    Some(vc_gui::Message::Go(Page::Settings))
                }
                keyboard::Key::Character("q") => Some(vc_gui::Message::Quit),
                keyboard::Key::Character("s") => Some(vc_gui::Message::StartSave),
                keyboard::Key::Character("f") => Some(vc_gui::Message::FocusSearch),
                keyboard::Key::Named(Named::Space) => Some(vc_gui::Message::TogglePower),
                _ => None,
            };
            match msg {
                Some(m) => app.ui(m),
                None => Task::none(),
            }
        }
        Message::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(keyboard::key::Named::Escape),
            ..
        }) => app.ui(vc_gui::Message::Escape),
        Message::Keyboard(_) => Task::none(),
        Message::Window(iced::window::Event::Resized(size)) => {
            if app.screenshot.is_none()
                && let Ok(mut c) = app.shared.config.lock()
            {
                let pos = c
                    .window
                    .map(|w| (w[0], w[1]))
                    .unwrap_or((f32::NAN, f32::NAN));
                c.window = Some([pos.0, pos.1, size.width, size.height]);
            }
            Task::none()
        }
        Message::Window(iced::window::Event::Moved(point)) => {
            if app.screenshot.is_none()
                && let Ok(mut c) = app.shared.config.lock()
            {
                let size = c
                    .window
                    .map(|w| (w[2], w[3]))
                    .unwrap_or((vc_gui::DEFAULT_WIDTH, vc_gui::DEFAULT_HEIGHT));
                c.window = Some([point.x, point.y, size.0, size.1]);
            }
            Task::none()
        }
        Message::Window(..) => Task::none(),
        Message::Shot(shot) => {
            if let Some((path, _, _)) = app.screenshot.take() {
                match save_png(&path, &shot) {
                    Ok(()) => log::info!("saved screenshot {}", path.display()),
                    Err(e) => log::error!("screenshot: {e}"),
                }
            }
            app.shared.request_quit();
            iced::exit()
        }
    }
}

fn save_png(path: &std::path::Path, shot: &window::Screenshot) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let img = image::RgbaImage::from_raw(shot.size.width, shot.size.height, shot.rgba.to_vec())
        .ok_or_else(|| anyhow::anyhow!("screenshot buffer has the wrong size"))?;
    img.save(path)?;
    Ok(())
}

fn view(app: &App, _id: window::Id) -> Element<'_, Message> {
    vc_gui::view(&app.model, &app.host).map(Message::Ui)
}

fn subscription(_app: &App) -> Subscription<Message> {
    Subscription::batch([
        iced::time::every(Duration::from_millis(16)).map(|_| Message::Tick),
        window::close_requests().map(Message::CloseRequested),
        keyboard::listen().map(Message::Keyboard),
        window::events().map(|(_, e)| Message::Window(e)),
    ])
}

/// Run the window until quit. Returns when the daemon exits.
pub fn run(shared: Arc<Shared>, options: Options) -> anyhow::Result<()> {
    let boot = move || {
        let mut model = Model::new();
        let screenshot = options.screenshot.clone().map(|(p, page)| {
            model.page = page;
            (p, page, 0u32)
        });
        let mut app = App {
            model,
            host: AppHost::new(shared.clone()),
            shared: shared.clone(),
            window: None,
            screenshot,
        };
        app.sync_from_shared();
        let task = if options.start_hidden && app.screenshot.is_none() {
            Task::none()
        } else {
            app.open_window()
        };
        (app, task)
    };
    iced::daemon(boot, update, view)
        .title(|_app: &App, _id| "Voice Changer".to_string())
        .theme(|app: &App, _id| -> Theme { vc_gui::theme::theme(app.host.theme_mode()) })
        .subscription(subscription)
        .antialiasing(true)
        .run()
        .map_err(|e| anyhow::anyhow!("window: {e}"))
}
