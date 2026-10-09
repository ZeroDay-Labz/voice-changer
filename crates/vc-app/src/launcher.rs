//! Make sure the desktop knows our icon. On Wayland the task bar and window
//! icon come from the installed `.desktop` entry (matched by app id) and the
//! hicolor icon theme, not from the window itself, so a `cargo run` or a
//! tarball install would show a placeholder. Unless a system-wide package
//! installed the entry, write a per-user copy of the launcher and icons.

use std::path::{Path, PathBuf};

const APP_ID: &str = "io.github.zerodaylabz.VoiceChanger";
const DESKTOP: &[u8] =
    include_bytes!("../../../packaging/io.github.zerodaylabz.VoiceChanger.desktop");
const SVG: &[u8] = include_bytes!("../../../packaging/io.github.zerodaylabz.VoiceChanger.svg");
const PNGS: [(u32, &[u8]); 7] = [
    (16, include_bytes!("../../../packaging/icons/16.png")),
    (32, include_bytes!("../../../packaging/icons/32.png")),
    (48, include_bytes!("../../../packaging/icons/48.png")),
    (64, include_bytes!("../../../packaging/icons/64.png")),
    (128, include_bytes!("../../../packaging/icons/128.png")),
    (256, include_bytes!("../../../packaging/icons/256.png")),
    (512, include_bytes!("../../../packaging/icons/512.png")),
];

fn data_home() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
        return Some(PathBuf::from(d));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))
}

fn write_if_different(path: &Path, bytes: &[u8]) -> std::io::Result<bool> {
    if std::fs::read(path).map(|cur| cur == bytes).unwrap_or(false) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)?;
    Ok(true)
}

/// The `Exec=` line: the bare name when `voice-changer` is on PATH, else
/// this very binary (so `cargo run` users get a working launcher too).
fn exec_line() -> String {
    let on_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join("voice-changer").is_file()))
        .unwrap_or(false);
    if on_path {
        "voice-changer".into()
    } else {
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "voice-changer".into())
    }
}

/// Install the launcher and icons for the current user if no package did.
/// Quiet and idempotent; failures are logged, never fatal.
pub fn ensure_installed() {
    if Path::new("/usr/share/applications")
        .join(format!("{APP_ID}.desktop"))
        .is_file()
    {
        return;
    }
    let Some(data) = data_home() else { return };
    let mut changed = false;
    let desktop = String::from_utf8_lossy(DESKTOP)
        .replace("Exec=voice-changer", &format!("Exec={}", exec_line()));
    match write_if_different(
        &data.join("applications").join(format!("{APP_ID}.desktop")),
        desktop.as_bytes(),
    ) {
        Ok(c) => changed |= c,
        Err(e) => log::warn!("could not write launcher: {e}"),
    }
    let icons = data.join("icons/hicolor");
    for (size, png) in PNGS {
        match write_if_different(&icons.join(format!("{size}x{size}/apps/{APP_ID}.png")), png) {
            Ok(c) => changed |= c,
            Err(e) => log::warn!("could not write icon: {e}"),
        }
    }
    if let Err(e) = write_if_different(&icons.join(format!("scalable/apps/{APP_ID}.svg")), SVG) {
        log::warn!("could not write icon: {e}");
    }
    if changed {
        log::info!("installed launcher and icons under {}", data.display());
        for (cmd, args) in [
            ("update-desktop-database", vec![data.join("applications")]),
            (
                "gtk-update-icon-cache",
                vec![PathBuf::from("-q"), icons.clone()],
            ),
        ] {
            let _ = std::process::Command::new(cmd)
                .args(&args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}
