//! Voice Changer standalone.
//!
//! Linux: captures your microphone through PipeWire, runs the engine, and
//! publishes the result as a virtual microphone named "Voice Changer Mic"
//! that Discord, SIP clients and OBS can select directly.
//!
//! Windows: you pick an input and an output device; the changed voice plays
//! on the output. No virtual device is created.
//!
//! Run with no arguments to start the app (window + tray). Subcommands talk
//! to the running instance (D-Bus on Linux, a local socket on Windows), e.g.
//! bind `voice-changer toggle` to a key.

// A window program on Windows: no console pops up. Output of the command
// line still reaches a terminal it was started from (see `attach_console`).
#![cfg_attr(windows, windows_subsystem = "windows")]

mod audio;
mod config;
mod control;
#[cfg(target_os = "linux")]
mod dbus;
#[cfg(windows)]
mod desktop;
mod doctor;
mod gui_ctx;
#[cfg(target_os = "linux")]
mod hotkey;
mod ipc;
#[cfg(target_os = "linux")]
mod launcher;
mod logger;
mod selftest;
#[cfg(target_os = "linux")]
mod tray;
mod window;

#[cfg(not(any(target_os = "linux", windows)))]
compile_error!("Voice Changer builds for Linux and Windows.");

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use std::sync::atomic::Ordering;
use vc_core::{Preset, VcParams};

use crate::config::AppConfig;
use crate::control::Shared;

#[derive(Parser, Debug)]
#[command(name = "voice-changer", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Microphone to capture (a name from `voice-changer mics`). Overrides the saved choice.
    #[arg(long)]
    mic: Option<String>,

    /// Sample rate to run the engine at. 48000 is what the effects and the AI are tuned for.
    #[arg(long, default_value_t = 48_000)]
    rate: u32,

    /// Requested block size in frames. 256 is safe for USB headsets; 128
    /// shaves 2.7 ms if your interface copes.
    /// Overrides the Buffer setting.
    #[arg(long)]
    quantum: Option<u32>,

    /// Start bypassed (passes the mic through untouched).
    #[arg(long)]
    bypass: bool,

    /// Start enabled even if the last session ended bypassed.
    #[arg(long, hide = true, conflicts_with = "bypass")]
    on: bool,

    /// Start hidden in the tray.
    #[arg(long)]
    minimized: bool,

    /// Apply this preset on start (factory or user preset name).
    #[arg(long)]
    preset: Option<String>,

    /// Don't open a window or tray; just run the audio (headless).
    #[arg(long)]
    headless: bool,

    /// AI voice model (.onnx path or name from the voices folder) to enable on start.
    #[arg(long)]
    ai_voice: Option<String>,

    /// Don't register on D-Bus (allows a second, independent instance for testing).
    #[arg(long, hide = true)]
    no_dbus: bool,

    /// Test input: loop a mono WAV (16-bit or float, any rate ≈ 48 kHz) instead of the mic;
    /// "synth" loops a built-in synthetic voice (no file needed).
    #[arg(long, hide = true)]
    input_wav: Option<std::path::PathBuf>,

    /// Start with the headphone monitor on.
    #[arg(long)]
    monitor: bool,

    /// Replay this WAV through the chain to "Hear myself" only (test bench).
    #[arg(long, hide = true)]
    bench_play: Option<std::path::PathBuf>,

    /// Record a bench clip with this name right after start.
    #[arg(long, hide = true)]
    bench_record: Option<String>,

    /// Play "Hear myself" to this sink (node.name) instead of the default output.
    #[arg(long, hide = true)]
    monitor_target: Option<String>,

    /// PipeWire node name of the published microphone.
    #[arg(long, hide = true, default_value = audio::SOURCE_NODE_NAME)]
    source_name: String,

    /// Render the window to this PNG and quit (for documentation).
    #[arg(long, value_name = "FILE")]
    screenshot: Option<std::path::PathBuf>,

    /// Page to show for --screenshot: home, voices, effects, mixer, settings.
    #[arg(long, default_value = "home")]
    page: String,

    /// AI inference threads: a number, or "auto" (derived from your CPU). Overrides the saved setting.
    #[arg(long, value_name = "N|auto")]
    ai_threads: Option<String>,

    /// Run the engine through every preset on synthetic speech and exit (no audio devices needed).
    #[arg(long)]
    self_test: bool,

    /// Check that the AI engine loads and runs a test model (no voice needed), then exit.
    #[arg(long, hide = true)]
    ai_self_test: bool,
}

#[derive(Subcommand, Debug)]
enum VoicesAction {
    /// Download (and convert if needed) a voice from a Hugging Face page or a direct .pth/.onnx/.zip link.
    Add { url: String, name: Option<String> },
    /// Delete an installed voice by name.
    Remove { name: String },
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Flip processing on/off in the running instance.
    Toggle,
    /// Turn processing on.
    On,
    /// Turn processing off (bypass).
    Off,
    /// Print whether processing is on.
    Status,
    /// Silence the output: `mute on`, `mute off`, or `mute` alone to flip it.
    Mute { state: Option<String> },
    /// Apply a preset in the running instance.
    Preset { name: String },
    /// List available presets.
    Presets,
    /// Hear yourself: play the processed voice to the default output (on|off).
    Monitor { state: String },
    /// Pick an AI voice model by name or path ("off" to disable AI), optionally with a speaker number (1-based).
    Ai { voice: String, speaker: Option<u32> },
    /// List AI voice models and show AI status.
    AiVoices,
    /// List microphones (node.name and description).
    Mics,
    /// Switch microphone by node.name ("default" for the system default).
    Mic { name: String },
    /// List output devices (name and description).
    Outputs,
    /// Switch the output device by name ("default" for the system default).
    Output { name: String },
    /// Manage installed AI voices: `voices`, `voices add <url> [name]`, `voices remove <name>`.
    Voices {
        #[command(subcommand)]
        action: Option<VoicesAction>,
    },
    /// List applications that are recording and whether they get the voice.
    Apps,
    /// Send the voice to an application (`route Discord on`) or release it.
    Route { app: String, state: String },
    /// Use the virtual mic as the default microphone for everything (on|off).
    Default { state: String },
    /// Bring the window to the front.
    Show,
    /// Quit the running instance.
    Quit,
    /// Check this computer: PipeWire, session manager, PulseAudio apps, microphones, GPU, models, tray, shortcut portal.
    Doctor,
    /// Download the AI engine (ONNX Runtime) into ~/.local/lib/voice-changer: the CPU build, or with --cuda the NVIDIA build.
    GetRuntime {
        /// The NVIDIA build instead of the CPU one, with NVIDIA's CUDA 12 and cuDNN 9 libraries (up to 1.7 GB; only the driver has to be installed).
        #[arg(long)]
        cuda: bool,
    },
}

/// Load the AI engine and run a tiny built-in model: on the processor, and
/// on the graphics card where this machine and this build have one to offer.
#[cfg(feature = "ai")]
fn ai_self_test() -> Result<()> {
    use vc_core::ai::{compute, rvc};
    println!("processor: {}", rvc::probe(compute::Backend::Cpu)?);
    let mut gpu = compute::resolve("auto");
    if gpu == compute::Backend::Cpu && compute::runtime_kind() == compute::RuntimeKind::DirectMl {
        // DirectML is never picked by Auto; try it because we were asked to.
        gpu = compute::Backend::DirectMl { device: 0 };
    }
    if gpu != compute::Backend::Cpu {
        match rvc::probe(gpu) {
            Ok(line) => println!("graphics card: {line}"),
            Err(e) => println!("graphics card: not usable ({e:#})"),
        }
        if let Some(why) = compute::gpu_failure() {
            println!("graphics card: {why}");
        }
    }
    Ok(())
}

#[cfg(not(feature = "ai"))]
fn ai_self_test() -> Result<()> {
    anyhow::bail!("this build has no AI support")
}

/// Audio block size in frames unless the user chose another.
const DEFAULT_QUANTUM: u32 = 256;

/// Whether the remote control uses the local socket rather than D-Bus:
/// always off Linux, and on Linux when `VC_IPC=socket` asks for it (tests).
fn use_socket() -> bool {
    cfg!(not(target_os = "linux")) || std::env::var("VC_IPC").is_ok_and(|v| v == "socket")
}

/// A window program has no console on Windows. When it was started from a
/// terminal, borrow that one so `voice-changer status`, `doctor` and
/// `--help` print where the user typed them.
#[cfg(windows)]
fn attach_console() {
    unsafe extern "system" {
        fn AttachConsole(process: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    // SAFETY: a plain Win32 call; failing (no parent console) changes nothing.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

fn main() -> Result<()> {
    #[cfg(windows)]
    attach_console();
    logger::init(
        "info,zbus=error,tracing=error,ksni=warn,ashpd=warn,ort=error,wgpu_core=warn,wgpu_hal=warn,naga=warn,iced_wgpu=warn,iced_winit=warn,winit=warn,cosmic_text=warn,sctk=warn",
    );
    let cli = Cli::parse();

    if let Some(Command::Voices { action }) = &cli.command {
        return manage_voices(action.as_ref());
    }
    if let Some(Command::Doctor) = &cli.command {
        let code = doctor::print(&doctor::run_checks());
        std::process::exit(code);
    }
    if let Some(Command::GetRuntime { cuda }) = &cli.command {
        return get_runtime(*cuda);
    }
    if cli.self_test {
        return selftest::run(cli.rate, cli.quantum.unwrap_or(DEFAULT_QUANTUM));
    }
    if cli.ai_self_test {
        return ai_self_test();
    }
    if let Some(cmd) = cli.command {
        return remote(cmd);
    }

    // Background services (D-Bus, tray, hotkey) on a tokio runtime thread.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    // A second launch brings the running window to the front instead of
    // creating a second virtual microphone.
    if !cli.no_dbus && cli.screenshot.is_none() && show_running_instance(&runtime) {
        log::info!("Voice Changer is already running; showing its window");
        std::process::exit(0);
    }

    let mut cfg = AppConfig::load();
    if cli.mic.is_some() {
        cfg.mic = cli.mic.clone();
    }
    if let Some(t) = &cli.ai_threads {
        cfg.ai_threads = match t.trim().to_ascii_lowercase().as_str() {
            "auto" | "0" => 0,
            n => n
                .parse::<u32>()
                .with_context(|| format!("--ai-threads: {t:?} is not a number or \"auto\""))?,
        };
    }
    #[cfg(feature = "ai")]
    vc_core::ai::compute::probe_in_background();

    log::info!("audio library: {}", audio::library_version());
    let params = VcParams::new();

    // Restore last state, then preset / flags from the command line.
    if let Some(state) = config::load_state() {
        state.apply_direct(&params, cli.rate as f32);
    }
    params.set_ai_threads(cfg.ai_threads);
    params.set_my_pitch_hz(cfg.my_pitch_hz);
    let quantum = cli
        .quantum
        .or((cfg.buffer > 0).then_some(cfg.buffer))
        .unwrap_or(DEFAULT_QUANTUM)
        .clamp(64, 4096);
    let audio_config = audio::AudioConfig {
        rate: cli.rate,
        quantum,
        capture_target: cfg.mic.clone(),
        source_name: cli.source_name.clone(),
        input_wav: match &cli.input_wav {
            Some(p) if p.as_os_str() == "synth" => Some(std::sync::Arc::new(
                selftest::synth_voice(cli.rate as usize, 4.0),
            )),
            Some(p) => Some(std::sync::Arc::new(vc_core::wav::read_mono(p)?.0)),
            None => None,
        },
        output_target: cli.monitor_target.clone().or_else(|| cfg.output.clone()),
    };
    let stats = std::sync::Arc::new(audio::Stats::default());
    let meters = std::sync::Arc::new(vc_core::Meters::default());
    let audio = match audio::AudioThread::spawn(
        audio_config.clone(),
        params.clone(),
        stats.clone(),
        meters.clone(),
    ) {
        Ok(a) => Ok(a),
        Err(e) => {
            // Explain in plain words; the window shows this with a Retry
            // button, headless runs stop here.
            let hint = doctor::audio_hint(&format!("{e:#}"));
            log::error!("audio could not start: {e:#}. {hint}");
            if cli.headless {
                anyhow::bail!("{hint}");
            }
            Err(hint)
        }
    };
    let shared = Shared::new(
        params,
        cli.rate as f32,
        quantum,
        audio,
        audio_config,
        stats,
        meters,
        cfg.clone(),
    );
    install_panic_hook(&shared);
    // A preset named on the command line is applied. Otherwise the restored
    // state is authoritative (it keeps the user's own tweaks), so we only
    // *name* the last preset when the parameters still match it -- the UI must
    // never claim a preset that is not actually in effect.
    let mut shown_preset: Option<String> = None;
    if let Some(name) = cli.preset.as_deref() {
        if shared.apply_preset_named(name) {
            shown_preset = Some(name.to_string());
        } else {
            log::warn!("unknown preset {name:?}");
        }
    } else if let Some(name) = cfg.last_preset.as_deref() {
        let still_matches = vc_core::presets::list_presets()
            .into_iter()
            .find(|e| e.preset.name.eq_ignore_ascii_case(name))
            .is_some_and(|e| e.preset.matches(&shared.params));
        if still_matches {
            shown_preset = Some(name.to_string());
        }
    }
    if let Ok(mut cur) = shared.current_preset.lock() {
        *cur = shown_preset;
    }
    shared.set_enabled(cli.on || !(cli.bypass || cfg.start_bypassed));
    // Mute is for the moment: never start silent because the last session
    // ended that way.
    shared.set_muted(false);
    if cli.monitor {
        shared.set_monitor(true);
    }
    if let Some(path) = &cli.bench_play
        && !shared.bench_play_file(path)
    {
        log::warn!("could not play the bench clip {}", path.display());
    }
    if let Some(name) = &cli.bench_record {
        shared.bench_record(name);
    }
    if cfg.set_as_default {
        // Applied by the audio thread once the session manager is known.
        shared.set_default_source(true);
    }
    if let Some(voice) = &cli.ai_voice
        && !shared.set_ai_voice(voice)
    {
        log::warn!("unknown AI voice {voice:?}");
    }

    let start_hidden = cli.minimized || cfg.start_minimized;
    let services = {
        let shared = shared.clone();
        let hotkey = cfg.hotkey.clone();
        let mute_hotkey = cfg.mute_hotkey.clone();
        let headless = cli.headless;
        let no_dbus = cli.no_dbus;
        runtime.spawn(async move {
            #[cfg(target_os = "linux")]
            let _conn = if no_dbus || use_socket() {
                None
            } else {
                match dbus::serve(shared.clone()).await {
                    Ok(c) => Some(c),
                    Err(zbus::Error::NameTaken) => {
                        log::error!("another Voice Changer instance is already running; use `voice-changer show`");
                        shared.request_quit();
                        return;
                    }
                    Err(e) => {
                        log::warn!("D-Bus control unavailable: {e}");
                        None
                    }
                }
            };
            if !no_dbus
                && use_socket()
                && let Err(e) = ipc::serve(shared.clone())
            {
                log::error!("{e:#}; use `voice-changer show`");
                shared.request_quit();
                return;
            }
            if headless {
                while !shared.quit.load(Ordering::SeqCst) {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
                return;
            }
            // The tray and the hotkey are independent: a desktop without a
            // tray host must not lose the global shortcut.
            #[cfg(target_os = "linux")]
            {
                let tray = tokio::spawn(tray::run(shared.clone(), start_hidden));
                let keys = tokio::spawn(hotkey::run(shared.clone(), hotkey, mute_hotkey));
                let _ = tray.await;
                let _ = keys.await;
            }
            // Elsewhere the tray and the shortcuts live with the window
            // (see `desktop.rs`).
            #[cfg(not(target_os = "linux"))]
            {
                let _ = (hotkey, mute_hotkey, start_hidden);
                while !shared.quit.load(Ordering::SeqCst) {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            }
        })
    };

    {
        // Ctrl-C / SIGTERM: save state and tear down cleanly in both modes.
        let running = shared.clone();
        ctrlc::set_handler(move || running.request_quit())?;
    }
    if cli.headless {
        log::info!("headless; `voice-changer toggle` or Ctrl-C to control");
        while !shared.quit.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    } else {
        #[cfg(target_os = "linux")]
        if !cli.no_dbus {
            launcher::ensure_installed();
        }
        let page = match cli.page.to_ascii_lowercase().as_str() {
            "voices" => vc_gui::Page::Voices,
            "effects" => vc_gui::Page::Effects,
            "mixer" => vc_gui::Page::Mixer,
            "settings" => vc_gui::Page::Settings,
            _ => vc_gui::Page::Home,
        };
        // A window that cannot open (no display, broken GPU stack) must not
        // skip the state save and the microphone restore below.
        if let Err(e) = window::run(
            shared.clone(),
            window::Options {
                start_hidden,
                screenshot: cli.screenshot.clone().map(|p| (p, page)),
            },
        ) {
            log::error!(
                "{e:#}. If you have no display, run with --headless; on a VM try ICED_BACKEND=tiny-skia."
            );
        }
        shared.request_quit();
    }

    // Persist state, then tear everything down.
    let mut cfg = shared.config.lock().map(|c| c.clone()).unwrap_or(cfg);
    cfg.last_preset = shared.current_preset.lock().ok().and_then(|c| c.clone());
    cfg.mic = shared
        .audio
        .lock()
        .ok()
        .and_then(|a| a.as_ref().and_then(|a| a.capture_target()))
        .or(cfg.mic);
    cfg.start_bypassed = !shared.is_enabled();
    cfg.ai_threads = shared.params.ai_threads();
    cfg.my_pitch_hz = shared.params.my_pitch_hz();
    // A bench recording still in flight is written out by this call.
    let _ = shared.bench_state();
    if let Err(e) = cfg.save() {
        log::warn!("could not save config: {e}");
    }
    if let Err(e) = config::save_state(&Preset::capture(&shared.params, "state")) {
        log::warn!("could not save state: {e}");
    }

    runtime.block_on(async {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), services).await;
    });
    runtime.shutdown_timeout(std::time::Duration::from_secs(1));

    let audio = shared.audio.lock().ok().and_then(|mut a| a.take());
    if let Some(audio) = audio {
        let stats = audio.stats.clone();
        if let Err(e) = audio.shutdown() {
            log::warn!("audio shutdown: {e:#}");
        }
        log::info!(
            "callbacks: capture={} output={} underruns={} trimmed_frames={} skipped={} process_avg={}us process_max={}us (quantum={}us)",
            stats.capture_callbacks.load(Ordering::Relaxed),
            stats.output_callbacks.load(Ordering::Relaxed),
            stats.underruns.load(Ordering::Relaxed),
            stats.trimmed_frames.load(Ordering::Relaxed),
            stats.skipped_callbacks.load(Ordering::Relaxed),
            stats.process_avg_us.load(Ordering::Relaxed),
            stats.process_max_us.load(Ordering::Relaxed),
            quantum as u64 * 1_000_000 / cli.rate as u64,
        );
    }
    // Everything is saved and torn down; skip slow Wayland/D-Bus destructors.
    std::process::exit(0);
}

/// Release builds abort on panic. Before that happens, give every
/// application its own microphone back and restore the default source, so
/// a crash never leaves the user's Discord pointed at a microphone that no
/// longer exists.
fn install_panic_hook(shared: &std::sync::Arc<Shared>) {
    static HOOKED: std::sync::OnceLock<std::sync::Weak<Shared>> = std::sync::OnceLock::new();
    let _ = HOOKED.set(std::sync::Arc::downgrade(shared));
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        if let Some(shared) = HOOKED.get().and_then(|w| w.upgrade())
            && let Ok(audio) = shared.audio.try_lock()
            && let Some(audio) = audio.as_ref()
        {
            log::error!("panic: restoring microphones before exiting");
            if audio.take_over() {
                audio.set_default_source(false);
                // Give the PipeWire thread a moment to apply it.
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
        }
    }));
}

/// Windows downloads carry the AI engine with them.
#[cfg(windows)]
fn get_runtime(_cuda: bool) -> Result<()> {
    println!(
        "The AI engine (onnxruntime.dll) comes with the Windows download, next to voice-changer.exe. There is nothing to fetch."
    );
    Ok(())
}

#[cfg(not(windows))]
/// Run the bundled download script (the same one as `scripts/get-onnxruntime.sh`),
/// so packaged installs have it too.
fn get_runtime(cuda: bool) -> Result<()> {
    use std::io::Write as _;
    const SCRIPT: &str = include_str!("../../../scripts/get-onnxruntime.sh");
    let mut child = std::process::Command::new("bash")
        .arg("-s")
        .arg("--")
        .args(cuda.then_some("--cuda"))
        .stdin(std::process::Stdio::piped())
        .spawn()
        .context("could not start bash")?;
    child
        .stdin
        .take()
        .context("no stdin for bash")?
        .write_all(SCRIPT.as_bytes())?;
    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!(
            "the download did not finish (it needs curl, tar and unzip, a network connection and, with --cuda, 3 GB of disk space); run it again to continue"
        );
    }
    Ok(())
}

/// Voice library operations work on files directly; no running instance needed.
#[cfg(feature = "ai")]
fn manage_voices(action: Option<&VoicesAction>) -> Result<()> {
    use vc_core::ai::library;
    match action {
        None => {
            for v in library::voices() {
                println!(
                    "{:<28} {:>5.0} MB {}",
                    v.name,
                    v.size_mb,
                    v.sample_rate
                        .map(|r| format!("{} kHz", r / 1000))
                        .unwrap_or_default()
                );
            }
            if let Some(dir) = vc_core::ai::voices_dir() {
                println!("({})", dir.display());
            }
        }
        Some(VoicesAction::Add { url, name }) => {
            let job = library::start_import(url.clone(), name.clone());
            let mut last = String::new();
            loop {
                let stage = job.stage();
                let line = match &stage {
                    vc_core::ai::Stage::Downloading { file, done, total } => format!(
                        "downloading {file}: {:.0} MB{}",
                        *done as f32 / 1e6,
                        total
                            .map(|t| format!(" / {:.0}", t as f32 / 1e6))
                            .unwrap_or_default()
                    ),
                    vc_core::ai::Stage::Extracting => "unpacking".into(),
                    vc_core::ai::Stage::PreparingConverter => {
                        "setting up converter (one-time)".into()
                    }
                    vc_core::ai::Stage::Converting => "converting to ONNX".into(),
                    vc_core::ai::Stage::Checking => "checking quality".into(),
                    vc_core::ai::Stage::Checked(result) => {
                        println!("\n{result}");
                        return Ok(());
                    }
                    vc_core::ai::Stage::Done(n) => {
                        println!("\ninstalled \"{n}\"");
                        return Ok(());
                    }
                    vc_core::ai::Stage::Failed(e) => anyhow::bail!("{e}"),
                    vc_core::ai::Stage::Idle => String::new(),
                };
                if line != last {
                    eprint!("\r\x1b[2K{line}");
                    last = line;
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        }
        Some(VoicesAction::Remove { name }) => {
            let v = library::voices()
                .into_iter()
                .find(|v| {
                    v.name.eq_ignore_ascii_case(name)
                        || v.path
                            .file_stem()
                            .is_some_and(|s| s.to_string_lossy() == *name)
                })
                .ok_or_else(|| anyhow::anyhow!("no voice named {name:?}"))?;
            library::delete_voice(&v.path)?;
            println!("removed {}", v.name);
        }
    }
    Ok(())
}

#[cfg(not(feature = "ai"))]
fn manage_voices(_action: Option<&VoicesAction>) -> Result<()> {
    anyhow::bail!("this build has no AI support")
}

/// Bring the window of an instance that is already running to the front.
/// Returns false when there is none.
fn show_running_instance(runtime: &tokio::runtime::Runtime) -> bool {
    if use_socket() {
        let _ = runtime;
        return ipc::running() && ipc::send(&["show".to_string()]).is_ok();
    }
    #[cfg(target_os = "linux")]
    if let Ok(proxy) = runtime.block_on(dbus::client()) {
        let _ = runtime.block_on(proxy.show_window());
        return true;
    }
    false
}

/// Send one command to the running instance.
fn remote(cmd: Command) -> Result<()> {
    if use_socket() {
        let _ = cmd;
        // The running instance parses the same command line we were given.
        let args: Vec<String> = std::env::args().skip(1).collect();
        print!("{}", ipc::send(&args)?);
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    return remote_dbus(cmd);
    #[cfg(not(target_os = "linux"))]
    unreachable!("the socket is the only remote control here")
}

#[cfg(target_os = "linux")]
fn remote_dbus(cmd: Command) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let proxy = dbus::client()
            .await
            .context("no running Voice Changer instance (start `voice-changer` first)")?;
        match cmd {
            Command::Toggle => {
                let on = proxy.toggle().await?;
                println!("{}", if on { "on" } else { "off" });
            }
            Command::On => proxy.set_enabled(true).await?,
            Command::Off => proxy.set_enabled(false).await?,
            Command::Status => {
                println!("{}", if proxy.enabled().await? { "on" } else { "off" });
                let (ai, voice) = proxy.ai_status().await?;
                if !voice.is_empty() {
                    println!("ai: {ai} ({voice})");
                }
            }
            Command::Mute { state } => match state.as_deref() {
                None | Some("toggle") => {
                    let on = !proxy.muted().await?;
                    proxy.set_muted(on).await?;
                    println!("{}", if on { "muted" } else { "unmuted" });
                }
                Some(s) => {
                    let on = matches!(s.to_ascii_lowercase().as_str(), "on" | "1" | "true" | "yes");
                    proxy.set_muted(on).await?;
                }
            },
            Command::Monitor { state } => {
                let on = matches!(
                    state.to_ascii_lowercase().as_str(),
                    "on" | "1" | "true" | "yes"
                );
                proxy.set_monitor(on).await?;
            }
            Command::Ai { voice, speaker } => {
                if !proxy.set_ai_voice(&voice).await? {
                    anyhow::bail!("unknown AI voice {voice:?} (see `voice-changer ai-voices`)");
                }
                if let Some(n) = speaker {
                    proxy.set_ai_speaker(n.saturating_sub(1) as i32).await?;
                }
            }
            Command::AiVoices => {
                let (ai, current) = proxy.ai_status().await?;
                println!("status: {ai}");
                #[cfg(feature = "ai")]
                let speakers: std::collections::HashMap<String, (u32, u32)> =
                    vc_core::ai::library::voices()
                        .into_iter()
                        .map(|v| {
                            (
                                v.path.to_string_lossy().to_string(),
                                (v.speakers, v.default_speaker),
                            )
                        })
                        .collect();
                #[cfg(not(feature = "ai"))]
                let speakers: std::collections::HashMap<String, (u32, u32)> = Default::default();
                let cur_speaker = proxy.ai_speaker().await.unwrap_or(0);
                for (name, path) in proxy.ai_voices().await? {
                    let extra = match speakers.get(&path) {
                        Some((n, d)) if *n > 1 => format!("\t{n} speakers (starts as {})", d + 1),
                        _ => String::new(),
                    };
                    let active = if path == current {
                        format!("* (speaker {})", cur_speaker + 1)
                    } else {
                        " ".into()
                    };
                    println!("{active} {name}\t{path}{extra}");
                }
            }
            Command::Preset { name } => {
                if !proxy.load_preset(&name).await? {
                    anyhow::bail!("unknown preset {name:?}");
                }
            }
            Command::Presets => {
                for p in proxy.presets().await? {
                    println!("{p}");
                }
            }
            Command::Mics => {
                let current = proxy.microphone().await?;
                println!(
                    "{} default\tSystem default",
                    if current.is_empty() { "*" } else { " " }
                );
                for (name, desc) in proxy.microphones().await? {
                    println!("{} {name}\t{desc}", if name == current { "*" } else { " " });
                }
            }
            Command::Mic { name } => {
                let name = if name == "default" {
                    String::new()
                } else {
                    name
                };
                proxy.set_microphone(&name).await?;
            }
            Command::Outputs => {
                let current = proxy.output().await?;
                println!(
                    "{} default\tSystem default",
                    if current.is_empty() { "*" } else { " " }
                );
                for (name, desc) in proxy.outputs().await? {
                    println!("{} {name}\t{desc}", if name == current { "*" } else { " " });
                }
            }
            Command::Output { name } => {
                let name = if name == "default" {
                    String::new()
                } else {
                    name
                };
                proxy.set_output_device(&name).await?;
            }
            Command::Voices { .. } => unreachable!("handled before connecting"),
            Command::Apps => {
                for (app, media, routed) in proxy.applications().await? {
                    println!("{} {app}\t{media}", if routed { "*" } else { " " });
                }
                println!(
                    "default microphone: {}",
                    if proxy.default_source().await? {
                        "Voice Changer Mic"
                    } else {
                        "system"
                    }
                );
            }
            Command::Route { app, state } => {
                let on = matches!(
                    state.to_ascii_lowercase().as_str(),
                    "on" | "1" | "true" | "yes"
                );
                if !proxy.route_application(&app, on).await? {
                    anyhow::bail!(
                        "no recording application named {app:?} (see `voice-changer apps`)"
                    );
                }
            }
            Command::Default { state } => {
                let on = matches!(
                    state.to_ascii_lowercase().as_str(),
                    "on" | "1" | "true" | "yes"
                );
                proxy.set_default_source(on).await?;
            }
            Command::Show => proxy.show_window().await?,
            Command::Quit => proxy.quit().await?,
            Command::Doctor | Command::GetRuntime { .. } => {
                unreachable!("handled before connecting")
            }
        }
        Ok(())
    })
}
