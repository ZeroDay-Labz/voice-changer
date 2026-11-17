//! Remote control over a local socket (a named pipe on Windows): the
//! command line of a second `voice-changer <command>` is sent to the
//! running instance, which carries it out and answers with the text to
//! print. This is what Windows uses; Linux speaks D-Bus (`dbus.rs`) and
//! uses this only when asked to with `VC_IPC=socket` (tests).

use anyhow::{Context as _, Result, anyhow, bail};
use clap::Parser as _;
use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, ListenerOptions, Name, Stream, prelude::*,
};
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::sync::Arc;

use crate::control::Shared;
use crate::{Cli, Command};

/// Separates the arguments of one command line on the wire.
const SEP: char = '\u{1f}';

/// One socket per user (and per `VC_IPC_NAME`, for tests).
fn socket_name() -> Result<Name<'static>> {
    let who = std::env::var("VC_IPC_NAME")
        .or_else(|_| std::env::var("USERNAME"))
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "user".into());
    let who: String = who
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let name = format!("voice-changer-{who}.sock");
    if GenericNamespaced::is_supported() {
        Ok(name.to_ns_name::<GenericNamespaced>()?.into_owned())
    } else {
        let path = std::env::temp_dir().join(name);
        Ok(path.to_fs_name::<GenericFilePath>()?.into_owned())
    }
}

/// Whether an instance is already running.
pub fn running() -> bool {
    socket_name()
        .ok()
        .is_some_and(|n| Stream::connect(n).is_ok())
}

/// Start answering commands. Fails when another instance owns the socket.
pub fn serve(shared: Arc<Shared>) -> Result<()> {
    let listener = ListenerOptions::new()
        .name(socket_name()?)
        .create_sync()
        .context("another Voice Changer instance is already running")?;
    std::thread::Builder::new()
        .name("remote-control".into())
        .spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { continue };
                let mut conn = BufReader::new(conn);
                let mut line = String::new();
                if conn.read_line(&mut line).is_err() {
                    continue;
                }
                let line = line.trim_end_matches(['\r', '\n']);
                // An empty line is the "are you there" of `running()`.
                if line.is_empty() {
                    continue;
                }
                let args: Vec<&str> = line.split(SEP).collect();
                let reply = match execute(&shared, &args) {
                    Ok(text) => format!("0\n{text}"),
                    Err(e) => format!("1\n{e:#}"),
                };
                let _ = conn.get_mut().write_all(reply.as_bytes());
            }
        })?;
    Ok(())
}

/// Send a command line to the running instance and return what it printed.
pub fn send(args: &[String]) -> Result<String> {
    let mut conn = Stream::connect(socket_name()?)
        .context("no running Voice Changer instance (start `voice-changer` first)")?;
    conn.write_all(format!("{}\n", args.join(&SEP.to_string())).as_bytes())?;
    let mut reply = String::new();
    conn.read_to_string(&mut reply)?;
    let (code, text) = reply.split_once('\n').unwrap_or(("1", "no answer"));
    if code == "0" {
        Ok(text.to_string())
    } else {
        Err(anyhow!("{text}"))
    }
}

fn on_off(state: &str) -> bool {
    matches!(
        state.to_ascii_lowercase().as_str(),
        "on" | "1" | "true" | "yes"
    )
}

fn device_list(devices: Vec<crate::audio::DeviceInfo>, current: Option<String>) -> String {
    let mut out = format!(
        "{} default\tSystem default\n",
        if current.is_none() { "*" } else { " " }
    );
    for d in devices {
        let mark = if Some(&d.name) == current.as_ref() {
            "*"
        } else {
            " "
        };
        out.push_str(&format!("{mark} {}\t{}\n", d.name, d.description));
    }
    out
}

fn device_choice(name: &str) -> Option<String> {
    (name != "default" && !name.is_empty()).then(|| name.to_string())
}

/// Carry out one command line (`["toggle"]`, `["preset", "Robot"]`…) on the
/// running instance. Returns the text for the caller's terminal.
pub fn execute(shared: &Arc<Shared>, args: &[&str]) -> Result<String> {
    let cli = Cli::try_parse_from(std::iter::once("voice-changer").chain(args.iter().copied()))
        .map_err(|e| anyhow!("{e}"))?;
    let Some(cmd) = cli.command else {
        bail!("no command");
    };
    let mut out = String::new();
    match cmd {
        Command::Toggle => out = if shared.toggle() { "on\n" } else { "off\n" }.into(),
        Command::On => shared.set_enabled(true),
        Command::Off => shared.set_enabled(false),
        Command::Status => {
            out.push_str(if shared.is_enabled() { "on\n" } else { "off\n" });
            if shared.muted() {
                out.push_str("muted\n");
            }
            let (ai, voice) = shared.ai_status_text();
            if !voice.is_empty() {
                out.push_str(&format!("ai: {ai} ({voice})\n"));
            }
        }
        Command::Mute { state } => match state.as_deref() {
            None | Some("toggle") => {
                let on = !shared.muted();
                shared.set_muted(on);
                out = if on { "muted\n" } else { "unmuted\n" }.into();
            }
            Some(s) => shared.set_muted(on_off(s)),
        },
        Command::Monitor { state } => shared.set_monitor(on_off(&state)),
        Command::Ai { voice, speaker } => {
            if !shared.set_ai_voice(&voice) {
                bail!("unknown AI voice {voice:?} (see `voice-changer ai-voices`)");
            }
            if let Some(n) = speaker {
                shared.set_param(
                    &shared.params.ai_speaker,
                    (n.saturating_sub(1) as i32).clamp(0, 15),
                );
            }
        }
        Command::AiVoices => {
            let (ai, current) = shared.ai_status_text();
            out.push_str(&format!("status: {ai}\n"));
            for (name, path) in shared.ai_voices() {
                let mark = if path == current { "*" } else { " " };
                out.push_str(&format!("{mark} {name}\t{path}\n"));
            }
        }
        Command::Preset { name } => {
            if !shared.apply_preset_named(&name) {
                bail!("unknown preset {name:?}");
            }
        }
        Command::Presets => {
            for p in vc_core::presets::list_presets() {
                out.push_str(&p.preset.name);
                out.push('\n');
            }
        }
        Command::Mics => {
            let current = shared
                .audio
                .lock()
                .ok()
                .and_then(|a| a.as_ref().and_then(|a| a.capture_target()));
            out = device_list(shared.devices(), current);
        }
        Command::Mic { name } => shared.set_capture_target(device_choice(&name)),
        Command::Outputs => out = device_list(shared.output_devices(), shared.output_target()),
        Command::Output { name } => shared.set_output_target(device_choice(&name)),
        Command::Apps => {
            for s in shared.app_streams() {
                let mark = if s.routed { "*" } else { " " };
                out.push_str(&format!("{mark} {}\t{}\n", s.app, s.media));
            }
        }
        Command::Route { app, state } => {
            if !shared.route_app_named(&app, on_off(&state)) {
                bail!("no recording application matches {app:?}");
            }
        }
        Command::Default { state } => shared.set_default_source(on_off(&state)),
        Command::Show => shared.request_show(),
        Command::Quit => shared.request_quit(),
        Command::Voices { .. } | Command::Doctor | Command::GetRuntime { .. } => {
            bail!("this command does not need a running instance")
        }
    }
    Ok(out)
}
