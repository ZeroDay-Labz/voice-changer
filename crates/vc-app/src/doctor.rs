//! `voice-changer doctor`: checks the machine the way a support person
//! would, and says in one line per item what is wrong and how to fix it.
//! The window shows the same findings as sentences when audio fails.

use pipewire as pw;
use pw::types::ObjectType;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub level: Level,
    pub what: String,
    pub detail: String,
    /// How to fix it, empty when nothing is wrong.
    pub fix: String,
}

impl Finding {
    fn ok(what: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            level: Level::Ok,
            what: what.into(),
            detail: detail.into(),
            fix: String::new(),
        }
    }
    fn warn(what: impl Into<String>, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            level: Level::Warn,
            what: what.into(),
            detail: detail.into(),
            fix: fix.into(),
        }
    }
    fn fail(what: impl Into<String>, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            level: Level::Fail,
            what: what.into(),
            detail: detail.into(),
            fix: fix.into(),
        }
    }
}

/// What a short look at the PipeWire graph revealed.
#[derive(Debug, Default, Clone)]
pub struct GraphProbe {
    pub connected: bool,
    pub error: Option<String>,
    pub server_version: Option<String>,
    pub has_null_sink_factory: bool,
    pub has_default_metadata: bool,
    pub sources: Vec<String>,
    pub sinks: usize,
    pub pulse_bridge: bool,
}

/// Connect, enumerate the registry for a moment, disconnect.
pub fn probe_graph(wait: Duration) -> GraphProbe {
    crate::pw::library_version();
    let probe = Rc::new(RefCell::new(GraphProbe::default()));
    let Ok(mainloop) = pw::main_loop::MainLoopRc::new(None) else {
        probe.borrow_mut().error = Some("could not create a PipeWire main loop".into());
        return probe.borrow().clone();
    };
    let Ok(context) = pw::context::ContextRc::new(&mainloop, None) else {
        probe.borrow_mut().error = Some("could not create a PipeWire context".into());
        return probe.borrow().clone();
    };
    let core = match context.connect_rc(None) {
        Ok(c) => c,
        Err(e) => {
            probe.borrow_mut().error = Some(format!("{e}"));
            return probe.borrow().clone();
        }
    };
    probe.borrow_mut().connected = true;
    let _core_listener = core
        .add_listener_local()
        .info({
            let probe = probe.clone();
            move |info| {
                probe.borrow_mut().server_version = Some(info.version().to_string());
            }
        })
        .register();
    let Ok(registry) = core.get_registry_rc() else {
        probe.borrow_mut().error = Some("could not get the registry".into());
        return probe.borrow().clone();
    };
    let _registry_listener = registry
        .add_listener_local()
        .global({
            let probe = probe.clone();
            move |global| {
                let props = global.props.as_ref();
                let mut p = probe.borrow_mut();
                match global.type_ {
                    ObjectType::Factory => {
                        // The virtual mic is an "adapter" wrapping the SPA
                        // support plugin's null sink; both must be there.
                        if props.and_then(|p| p.get("factory.name")) == Some("adapter") {
                            p.has_null_sink_factory = spa_support_plugin_present();
                        }
                    }
                    ObjectType::Metadata => {
                        if props.and_then(|p| p.get("metadata.name")) == Some("default") {
                            p.has_default_metadata = true;
                        }
                    }
                    ObjectType::Node => {
                        let class = props.and_then(|p| p.get("media.class")).unwrap_or("");
                        let name = props.and_then(|p| p.get("node.name")).unwrap_or("");
                        if (class == "Audio/Source" || class == "Audio/Source/Virtual")
                            && !name.starts_with("voice_changer")
                            && !name.starts_with("vc_")
                        {
                            let desc = props
                                .and_then(|p| p.get("node.description"))
                                .unwrap_or(name)
                                .to_string();
                            p.sources.push(desc);
                        } else if class == "Audio/Sink" {
                            p.sinks += 1;
                        }
                    }
                    ObjectType::Module => {
                        if props.and_then(|p| p.get("module.name"))
                            == Some("libpipewire-module-protocol-pulse")
                        {
                            p.pulse_bridge = true;
                        }
                    }
                    ObjectType::Client
                        if props.and_then(|p| p.get("application.name"))
                            == Some("pipewire-pulse") =>
                    {
                        p.pulse_bridge = true;
                    }
                    _ => {}
                }
            }
        })
        .register();
    let deadline = Instant::now() + wait;
    let timer = mainloop.loop_().add_timer({
        let mainloop = mainloop.clone();
        move |_| mainloop.quit()
    });
    let _ = timer.update_timer(
        Some(deadline.saturating_duration_since(Instant::now())),
        None,
    );
    mainloop.run();
    drop(timer);
    probe.borrow().clone()
}

/// `libspa-support.so` provides `support.null-audio-sink`. Minimal images
/// sometimes ship PipeWire without the SPA support plugins.
fn spa_support_plugin_present() -> bool {
    let dirs = [
        "/usr/lib64/spa-0.2/support",
        "/usr/lib/x86_64-linux-gnu/spa-0.2/support",
        "/usr/lib/aarch64-linux-gnu/spa-0.2/support",
        "/usr/lib/spa-0.2/support",
        "/usr/local/lib/spa-0.2/support",
        "/app/lib/spa-0.2/support",
    ];
    if let Ok(extra) = std::env::var("SPA_PLUGIN_DIR")
        && std::path::Path::new(&extra)
            .join("support/libspa-support.so")
            .is_file()
    {
        return true;
    }
    dirs.iter()
        .any(|d| std::path::Path::new(d).join("libspa-support.so").is_file())
}

/// "Server Name" from `pactl info`, if the PulseAudio API answers at all.
pub fn pulse_server_name() -> Option<String> {
    let out = std::process::Command::new("pactl")
        .arg("info")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("Server Name:").map(|v| v.trim().to_string()))
}

/// One sentence for the window when the audio side cannot start.
pub fn audio_hint(error: &str) -> String {
    let graph = probe_graph(Duration::from_millis(600));
    if !graph.connected {
        return match pulse_server_name() {
            Some(name) if !name.contains("PipeWire") => format!(
                "This computer uses PulseAudio ({name}) for sound, and Voice Changer needs PipeWire. On Ubuntu and Debian: install the pipewire-audio (or pipewire-pulse + wireplumber) package and log out and in again."
            ),
            _ => format!(
                "PipeWire does not seem to be running ({error}). On most desktops it starts with your session; try `systemctl --user start pipewire wireplumber`."
            ),
        };
    }
    if !graph.has_null_sink_factory {
        return "PipeWire is running but cannot create virtual devices (support.null-audio-sink is missing). Install the full pipewire package.".into();
    }
    format!("The audio system refused to start the voice changer ({error}).")
}

/// Run every check.
pub fn run_checks() -> Vec<Finding> {
    let mut out = Vec::new();

    // ---- display / desktop ----
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let x11 = std::env::var_os("DISPLAY").is_some();
    out.push(match (wayland, x11) {
        (true, _) => Finding::ok("Display", "Wayland"),
        (false, true) => Finding::ok("Display", "X11"),
        _ => Finding::warn(
            "Display",
            "no WAYLAND_DISPLAY or DISPLAY",
            "Run from a desktop session, or use --headless.",
        ),
    });
    if let Ok(backend) = std::env::var("ICED_BACKEND") {
        out.push(Finding::ok(
            "Renderer",
            format!("forced to {backend} (ICED_BACKEND)"),
        ));
    }

    // ---- audio ----
    let graph = probe_graph(Duration::from_millis(900));
    let pulse = pulse_server_name();
    match (&graph.connected, &pulse) {
        (false, Some(name)) if !name.contains("PipeWire") => out.push(Finding::fail(
            "Audio system",
            format!("PulseAudio is the sound server ({name}); PipeWire is not reachable"),
            "Install pipewire-audio (Debian/Ubuntu: `sudo apt install pipewire-audio`; Fedora has it already) and log out and in.",
        )),
        (false, _) => out.push(Finding::fail(
            "Audio system",
            format!(
                "PipeWire is not running{}",
                graph
                    .error
                    .as_deref()
                    .map(|e| format!(" ({e})"))
                    .unwrap_or_default()
            ),
            "Start it: `systemctl --user start pipewire wireplumber pipewire-pulse`.",
        )),
        (true, _) => {
            let ver = graph.server_version.clone().unwrap_or_else(|| "?".into());
            let lib = crate::pw::library_version();
            out.push(Finding::ok(
                "Audio system",
                format!("PipeWire {ver} (client library {lib})"),
            ));
            match &pulse {
                Some(name) if name.contains("PipeWire") => out.push(Finding::ok(
                    "PulseAudio apps",
                    "served by PipeWire (Discord, browsers, OBS will see the virtual mic)",
                )),
                Some(name) => out.push(Finding::fail(
                    "PulseAudio apps",
                    format!("sound is still served by {name}"),
                    "PipeWire only handles screen sharing on this system. Install pipewire-audio / pipewire-pulse and disable PulseAudio, then log out and in.",
                )),
                None => out.push(Finding::warn(
                    "PulseAudio apps",
                    "pactl is not installed, so this could not be checked",
                    "Install pulseaudio-utils (Debian/Ubuntu) or pipewire-pulseaudio (Fedora) if PulseAudio apps cannot see the virtual mic.",
                )),
            }
            if graph.has_default_metadata {
                out.push(Finding::ok("Session manager", "WirePlumber (or media-session) is running"));
            } else {
                out.push(Finding::fail(
                    "Session manager",
                    "no `default` metadata object: WirePlumber is not running",
                    "Start it: `systemctl --user start wireplumber`. Without it no microphone is connected and apps cannot be routed.",
                ));
            }
            if graph.has_null_sink_factory {
                out.push(Finding::ok("Virtual devices", "support.null-audio-sink is available"));
            } else {
                out.push(Finding::fail(
                    "Virtual devices",
                    "support.null-audio-sink factory is missing",
                    "Install the full pipewire package (some minimal images strip the SPA support plugins).",
                ));
            }
            if graph.sources.is_empty() {
                out.push(Finding::warn(
                    "Microphones",
                    "no audio source found",
                    "Plug in or enable a microphone; the voice changer will capture silence until then.",
                ));
            } else {
                out.push(Finding::ok(
                    "Microphones",
                    format!("{}: {}", graph.sources.len(), graph.sources.join(", ")),
                ));
            }
            if graph.sinks == 0 {
                out.push(Finding::warn(
                    "Outputs",
                    "no audio sink found",
                    "Hear myself has nowhere to play; the virtual microphone still works.",
                ));
            }
        }
    }

    // ---- AI ----
    #[cfg(feature = "ai")]
    {
        use vc_core::ai::{compute, hardware, library};
        match compute::runtime_lib() {
            Some((p, kind)) => {
                out.push(Finding::ok(
                    "ONNX Runtime",
                    format!("{} ({:?} build)", p.display(), kind),
                ));
                if let Some(note) = compute::rocm_note() {
                    out.push(Finding::warn("ROCm runtime", note, "Nothing to do unless this machine has an AMD GPU that ROCm should see; then check that the amdgpu driver and rocminfo work."));
                }
            }
            None => {
                if cfg!(feature = "gpu") {
                    out.push(Finding::warn(
                        "ONNX Runtime",
                        "no libonnxruntime.so found",
                        "AI voices need it. Reinstall the package, or run scripts/get-onnxruntime.sh (AMD: `dnf install onnxruntime-rocm`).",
                    ));
                } else {
                    out.push(Finding::ok("ONNX Runtime", "built in (CPU)"));
                }
            }
        }
        let gpus = compute::gpus();
        if gpus.is_empty() {
            out.push(Finding::ok(
                "GPU",
                compute::gpu_note().unwrap_or_else(|| "none; AI runs on the CPU".into()),
            ));
        } else {
            out.push(Finding::ok(
                "GPU",
                gpus.iter()
                    .map(|g| format!("{} ({})", g.name, g.api))
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }
        let info = hardware::info();
        let auto = hardware::recommended_threads(info, !gpus.is_empty());
        out.push(Finding::ok(
            "Processor",
            format!(
                "{} · AI uses {auto} thread{} on Auto",
                hardware::summary(),
                if auto == 1 { "" } else { "s" }
            ),
        ));
        if info.performance_cores <= 2 && gpus.is_empty() {
            out.push(Finding::warn(
                "AI on this CPU",
                "two cores or fewer",
                "AI voices will lag; the DSP presets (Man, Woman, Robot…) are instant.",
            ));
        }
        if library::base_models_present() {
            out.push(Finding::ok(
                "AI base models",
                "ContentVec and RMVPE are installed",
            ));
        } else {
            out.push(Finding::warn(
                "AI base models",
                "not downloaded",
                "Voices page → Download base models (about 740 MB, once).",
            ));
        }
        let voices = library::voices();
        out.push(Finding::ok(
            "AI voices",
            if voices.is_empty() {
                "none installed (Voices page → Add a voice)".to_string()
            } else {
                format!("{} installed", voices.len())
            },
        ));
    }

    // ---- desktop integration (D-Bus) ----
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok();
    if let Some(rt) = rt {
        let (tray, portal) = rt.block_on(async {
            let Ok(conn) = zbus::Connection::session().await else {
                return (None, None);
            };
            let Ok(dbus) = zbus::fdo::DBusProxy::new(&conn).await else {
                return (None, None);
            };
            let has = |name: &'static str| {
                let dbus = dbus.clone();
                async move {
                    dbus.name_has_owner(name.try_into().unwrap())
                        .await
                        .unwrap_or(false)
                }
            };
            (
                Some(has("org.kde.StatusNotifierWatcher").await),
                Some(has("org.freedesktop.portal.Desktop").await),
            )
        });
        out.push(match tray {
            Some(true) => Finding::ok("Tray", "a status-notifier host is running"),
            Some(false) => Finding::warn(
                "Tray",
                "no tray host (GNOME without the AppIndicator extension, some window managers)",
                "The window still works; close-to-tray and start-minimized keep the window open instead.",
            ),
            None => Finding::warn("Tray", "no session D-Bus", "Desktop integration (tray, hotkey, CLI) is unavailable."),
        });
        out.push(match portal {
            Some(true) => Finding::ok("Global shortcut", "desktop portal available"),
            Some(false) => Finding::warn(
                "Global shortcut",
                "no xdg-desktop-portal",
                "Bind `voice-changer toggle` to a key in your desktop's shortcut settings instead.",
            ),
            None => Finding::warn("Global shortcut", "no session D-Bus", ""),
        });
    }

    // ---- config dir ----
    match vc_core::presets::config_dir() {
        Some(d) => out.push(Finding::ok("Settings folder", d.display().to_string())),
        None => out.push(Finding::warn(
            "Settings folder",
            "no config directory (HOME unset?)",
            "Settings will not be saved.",
        )),
    }
    out
}

/// Print the findings; returns the exit code (1 when anything failed).
pub fn print(findings: &[Finding]) -> i32 {
    let mut failed = false;
    for f in findings {
        let tag = match f.level {
            Level::Ok => "OK  ",
            Level::Warn => "WARN",
            Level::Fail => {
                failed = true;
                "FAIL"
            }
        };
        println!("{tag}  {:<18} {}", f.what, f.detail);
        if !f.fix.is_empty() {
            println!("      {:<18} → {}", "", f.fix);
        }
    }
    println!();
    println!(
        "Voice Changer {} · {}",
        env!("CARGO_PKG_VERSION"),
        if failed {
            "something needs fixing before it can run"
        } else {
            "ready"
        }
    );
    if failed { 1 } else { 0 }
}
