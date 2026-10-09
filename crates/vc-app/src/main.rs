//! Voice Changer standalone: captures your microphone through PipeWire,
//! runs the engine, and publishes the result as a virtual microphone named
//! "Voice Changer Mic" that Discord, SIP clients and OBS can select directly.
//!
//! Run with no arguments to start the app (window + tray). Subcommands talk
//! to the running instance over D-Bus, e.g. bind `voice-changer toggle` to a
//! key in KDE System Settings → Shortcuts.

mod config;
mod control;
mod dbus;
mod gui_ctx;
mod hotkey;
mod launcher;
mod logger;
mod pw;
mod tray;
mod window;

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

    /// Microphone to capture (PipeWire `node.name`). Overrides the saved choice.
    #[arg(long)]
    mic: Option<String>,

    /// Sample rate to run the engine at. Match your PipeWire graph rate to avoid resampling.
    #[arg(long, default_value_t = 48_000)]
    rate: u32,

    /// Requested block size in frames (PipeWire quantum). 256 is safe for USB
    /// headsets; 128 shaves 2.7 ms if your interface copes.
    #[arg(long, default_value_t = 256)]
    quantum: u32,

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

    /// Test input: loop a mono WAV (16-bit or float, any rate ≈ 48 kHz) instead of the mic.
    #[arg(long, hide = true)]
    input_wav: Option<std::path::PathBuf>,

    /// Start with the headphone monitor on.
    #[arg(long)]
    monitor: bool,

    /// PipeWire node name of the published microphone.
    #[arg(long, hide = true, default_value = pw::SOURCE_NODE_NAME)]
    source_name: String,

    /// Render the window to this PNG and quit (for documentation).
    #[arg(long, value_name = "FILE")]
    screenshot: Option<std::path::PathBuf>,

    /// Page to show for --screenshot: home, voices, effects, mixer, settings.
    #[arg(long, default_value = "home")]
    page: String,
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
}

fn main() -> Result<()> {
    logger::init(
        "info,zbus=error,tracing=error,ksni=warn,ashpd=warn,ort=error,wgpu_core=warn,wgpu_hal=warn,naga=warn,iced_wgpu=warn,cosmic_text=warn,sctk=warn",
    );
    let cli = Cli::parse();

    if let Some(Command::Voices { action }) = &cli.command {
        return manage_voices(action.as_ref());
    }
    if let Some(cmd) = cli.command {
        return remote(cmd);
    }

    let mut cfg = AppConfig::load();
    if cli.mic.is_some() {
        cfg.mic = cli.mic.clone();
    }

    log::info!("PipeWire library {}", pw::library_version());
    let params = VcParams::new();

    // Restore last state, then preset / flags from the command line.
    if let Some(state) = config::load_state() {
        state.apply_direct(&params, cli.rate as f32);
    }
    let audio = pw::AudioThread::spawn(
        pw::AudioConfig {
            rate: cli.rate,
            quantum: cli.quantum,
            capture_target: cfg.mic.clone(),
            source_name: cli.source_name.clone(),
            input_wav: match &cli.input_wav {
                Some(p) => Some(std::sync::Arc::new(read_wav_mono(p)?)),
                None => None,
            },
        },
        params.clone(),
    )
    .context("start PipeWire audio")?;
    let shared = Shared::new(params, cli.rate as f32, cli.quantum, audio, cfg.clone());
    if let Some(name) = cli.preset.as_deref().or(cfg.last_preset.as_deref())
        && cli.preset.is_some()
        && !shared.apply_preset_named(name)
    {
        log::warn!("unknown preset {name:?}");
    }
    if let Ok(mut cur) = shared.current_preset.lock() {
        *cur = cfg.last_preset.clone();
    }
    shared.set_enabled(cli.on || !(cli.bypass || cfg.start_bypassed));
    if cli.monitor {
        shared.set_monitor(true);
    }
    if cfg.set_as_default {
        shared.set_default_source(true);
    }
    if let Some(voice) = &cli.ai_voice
        && !shared.set_ai_voice(voice)
    {
        log::warn!("unknown AI voice {voice:?}");
    }

    // Background services (D-Bus, tray, hotkey) on a tokio runtime thread.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let services = {
        let shared = shared.clone();
        let hotkey = cfg.hotkey.clone();
        let headless = cli.headless;
        let no_dbus = cli.no_dbus;
        runtime.spawn(async move {
            let _conn = match if no_dbus { Err(zbus::Error::Unsupported) } else { dbus::serve(shared.clone()).await } {
                Ok(c) => Some(c),
                Err(zbus::Error::Unsupported) => None,
                Err(zbus::Error::NameTaken) => {
                    log::error!("another Voice Changer instance is already running; use `voice-changer show`");
                    shared.request_quit();
                    return;
                }
                Err(e) => {
                    log::warn!("D-Bus control unavailable: {e}");
                    None
                }
            };
            if headless {
                while !shared.quit.load(Ordering::SeqCst) {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
                return;
            }
            let tray = tokio::spawn(tray::run(shared.clone()));
            let keys = tokio::spawn(hotkey::run(shared.clone(), hotkey));
            let _ = tray.await;
            keys.abort();
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
        window::run(
            shared.clone(),
            window::Options {
                start_hidden: cli.minimized || cfg.start_minimized,
                screenshot: cli.screenshot.clone().map(|p| (p, page)),
            },
        )?;
        shared.request_quit();
    }

    // Persist state, then tear everything down.
    let mut cfg = shared.config.lock().map(|c| c.clone()).unwrap_or(cfg);
    cfg.last_preset = shared.current_preset.lock().ok().and_then(|c| c.clone());
    cfg.mic = shared
        .audio
        .lock()
        .ok()
        .and_then(|a| a.as_ref().and_then(|a| a.capture_target()));
    cfg.start_bypassed = !shared.is_enabled();
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
        audio.shutdown()?;
        log::info!(
            "callbacks: capture={} output={} underruns={} trimmed_frames={} process_avg={}us process_max={}us (quantum={}us)",
            stats.capture_callbacks.load(Ordering::Relaxed),
            stats.output_callbacks.load(Ordering::Relaxed),
            stats.underruns.load(Ordering::Relaxed),
            stats.trimmed_frames.load(Ordering::Relaxed),
            stats.process_avg_us.load(Ordering::Relaxed),
            stats.process_max_us.load(Ordering::Relaxed),
            cli.quantum as u64 * 1_000_000 / cli.rate as u64,
        );
    }
    // Everything is saved and torn down; skip slow Wayland/D-Bus destructors.
    std::process::exit(0);
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

/// Send one command to the running instance.
fn remote(cmd: Command) -> Result<()> {
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
        }
        Ok(())
    })
}

/// Minimal WAV reader (PCM16 / float32, any channel count → mono).
fn read_wav_mono(path: &std::path::Path) -> Result<Vec<f32>> {
    let b = std::fs::read(path)?;
    let fmt = b
        .windows(4)
        .position(|w| w == b"fmt ")
        .context("wav: no fmt chunk")?;
    let format = u16::from_le_bytes([b[fmt + 8], b[fmt + 9]]);
    let channels = u16::from_le_bytes([b[fmt + 10], b[fmt + 11]]) as usize;
    let bits = u16::from_le_bytes([b[fmt + 22], b[fmt + 23]]);
    let data = b
        .windows(4)
        .position(|w| w == b"data")
        .context("wav: no data chunk")?;
    let size = u32::from_le_bytes([b[data + 4], b[data + 5], b[data + 6], b[data + 7]]) as usize;
    let pcm = &b[data + 8..(data + 8 + size).min(b.len())];
    let mut mono = Vec::new();
    match (format, bits) {
        (1, 16) => {
            for frame in pcm.chunks_exact(2 * channels) {
                let s: f32 = frame
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0)
                    .sum();
                mono.push(s / channels as f32);
            }
        }
        (3, 32) => {
            for frame in pcm.chunks_exact(4 * channels) {
                let s: f32 = frame
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| f32::from_le_bytes(*c))
                    .sum();
                mono.push(s / channels as f32);
            }
        }
        _ => anyhow::bail!("unsupported wav format {format}/{bits}"),
    }
    Ok(mono)
}
