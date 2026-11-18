//! Facts about this computer for the problem report a user can copy: the
//! system version and the graphics cards with their driver versions. The
//! questions someone helping would otherwise have to ask.

/// One fact per line; whatever cannot be found out is left out.
pub fn system_lines() -> Vec<String> {
    let mut out = Vec::new();
    if let Some(v) = system_version() {
        out.push(format!("System version: {v}"));
    }
    for g in graphics_cards() {
        out.push(format!("Graphics card: {g}"));
    }
    out
}

#[cfg(not(windows))]
fn system_version() -> Option<String> {
    let name = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                .map(|v| v.trim_matches('"').to_string())
        });
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|s| s.trim().to_string());
    match (name, kernel) {
        (Some(n), Some(k)) => Some(format!("{n}, kernel {k}")),
        (n, k) => n.or(k),
    }
}

#[cfg(not(windows))]
fn graphics_cards() -> Vec<String> {
    // Only what has already been looked up: asking now can take seconds.
    #[cfg(feature = "ai")]
    return vc_core::ai::compute::gpus_if_probed()
        .unwrap_or_default()
        .into_iter()
        .map(|g| format!("{} ({})", g.name, g.api))
        .collect();
    #[cfg(not(feature = "ai"))]
    Vec::new()
}

#[cfg(windows)]
fn system_version() -> Option<String> {
    let key = winreg::HKLM
        .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
        .ok()?;
    let build: String = key.get_value("CurrentBuild").ok()?;
    // Windows 11 still calls itself "Windows 10" in ProductName; the build
    // number is what tells them apart.
    let name = if build.parse::<u32>().is_ok_and(|b| b >= 22_000) {
        "Windows 11".to_string()
    } else {
        key.get_value("ProductName")
            .unwrap_or_else(|_| "Windows".to_string())
    };
    let release: String = key.get_value("DisplayVersion").unwrap_or_default();
    Some(format!("{name} {release} (build {build})").replace("  ", " "))
}

/// The display adapters Windows has drivers for, from the device class key
/// (no program is started, so nothing flashes on screen).
#[cfg(windows)]
fn graphics_cards() -> Vec<String> {
    const DISPLAY_CLASS: &str =
        r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";
    let Ok(class) = winreg::HKLM.open_subkey(DISPLAY_CLASS) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for name in class.enum_keys().flatten() {
        let Ok(adapter) = class.open_subkey(&name) else {
            continue;
        };
        let Ok(desc) = adapter.get_value::<String, _>("DriverDesc") else {
            continue;
        };
        let line = match adapter.get_value::<String, _>("DriverVersion") {
            Ok(v) => format!("{desc} (driver {v})"),
            Err(_) => desc,
        };
        if !out.contains(&line) {
            out.push(line);
        }
    }
    out
}
