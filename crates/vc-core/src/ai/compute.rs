//! Where the AI models run. Detected at startup; the user picks Auto, CPU
//! or a specific GPU and the worker reloads the models accordingly.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuInfo {
    pub index: i32,
    pub name: String,
    /// "ROCm" for now; CUDA/DirectML can slot in later.
    pub api: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Cpu,
    Rocm { device: i32 },
}

impl Backend {
    pub fn label(self, gpus: &[GpuInfo]) -> String {
        match self {
            Backend::Cpu => "CPU".into(),
            Backend::Rocm { device } => gpus
                .iter()
                .find(|g| g.index == device)
                .map(|g| format!("GPU {} ({})", g.index, g.name))
                .unwrap_or_else(|| format!("GPU {device} (ROCm)")),
        }
    }
}

/// The setting string stored in the parameters: "auto", "cpu", "rocm:N".
pub fn parse_setting(s: &str) -> Option<Backend> {
    match s.trim() {
        "" | "auto" => None,
        "cpu" => Some(Backend::Cpu),
        other => other
            .strip_prefix("rocm:")
            .and_then(|n| n.parse().ok())
            .map(|device| Backend::Rocm { device }),
    }
}

pub fn setting_string(b: Option<Backend>) -> String {
    match b {
        None => "auto".into(),
        Some(Backend::Cpu) => "cpu".into(),
        Some(Backend::Rocm { device }) => format!("rocm:{device}"),
    }
}

/// Fedora's `onnxruntime-rocm` install location (override with ORT_DYLIB_PATH).
pub fn rocm_runtime_lib() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ORT_DYLIB_PATH") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let candidates = ["/usr/lib64/rocm/lib", "/usr/lib/rocm/lib", "/opt/rocm/lib"];
    for dir in candidates {
        if let Ok(entries) = std::fs::read_dir(dir) {
            let mut libs: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .map(|f| f.to_string_lossy().starts_with("libonnxruntime.so"))
                        .unwrap_or(false)
                })
                .collect();
            libs.sort();
            if let Some(lib) = libs.pop() {
                return Some(lib);
            }
        }
    }
    None
}

/// Whether this build can use a GPU at all (ROCm-capable runtime present). Cached.
pub fn gpu_runtime_available() -> bool {
    static CACHE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHE.get_or_init(|| cfg!(feature = "gpu-rocm") && rocm_runtime_lib().is_some())
}

fn newest_lib(dir: &std::path::Path) -> Option<PathBuf> {
    let mut libs: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .map(|f| f.to_string_lossy().starts_with("libonnxruntime.so"))
                .unwrap_or(false)
        })
        .collect();
    libs.sort();
    libs.pop()
}

/// A CPU-only ONNX Runtime library for builds that load it dynamically:
/// next to the executable, in the package's private lib dir, in the user's
/// `~/.local/lib/voice-changer`, or the distribution's own package.
pub fn cpu_runtime_lib() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    {
        dirs.push(exe_dir);
    }
    dirs.push("/usr/lib64/voice-changer".into());
    dirs.push("/usr/lib/voice-changer".into());
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/lib/voice-changer"));
    }
    dirs.push("/usr/lib64".into());
    dirs.push("/usr/lib/x86_64-linux-gnu".into());
    dirs.push("/usr/lib".into());
    dirs.iter().find_map(|d| newest_lib(d))
}

/// The runtime library a dynamic build will load and whether it is the ROCm one.
pub fn runtime_lib() -> Option<(PathBuf, bool)> {
    if let Some(p) = rocm_runtime_lib() {
        return Some((p, true));
    }
    cpu_runtime_lib().map(|p| (p, false))
}

/// One line for the Settings page: which ONNX Runtime is in use.
pub fn runtime_description() -> String {
    if !cfg!(feature = "gpu-rocm") {
        return "ONNX Runtime built in (CPU)".to_string();
    }
    match runtime_lib() {
        Some((p, true)) => format!("ONNX Runtime with ROCm: {}", p.display()),
        Some((p, false)) => format!("ONNX Runtime (CPU): {}", p.display()),
        None => "ONNX Runtime not found: install onnxruntime or onnxruntime-rocm".to_string(),
    }
}

/// GPUs visible to ROCm, via `rocminfo` (no HIP linkage needed). Cached.
pub fn gpus() -> Vec<GpuInfo> {
    static CACHE: std::sync::OnceLock<Vec<GpuInfo>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            if !gpu_runtime_available() {
                return Vec::new();
            }
            let Ok(out) = std::process::Command::new("rocminfo").output() else {
                return Vec::new();
            };
            let text = String::from_utf8_lossy(&out.stdout);
            let mut gpus = Vec::new();
            let mut name: Option<String> = None;
            let mut marketing: Option<String> = None;
            let mut is_gpu = false;
            let mut index = 0;
            let flush = |gpus: &mut Vec<GpuInfo>,
                         name: &mut Option<String>,
                         marketing: &mut Option<String>,
                         is_gpu: &mut bool,
                         index: &mut i32| {
                if *is_gpu {
                    let label = marketing
                        .take()
                        .or_else(|| name.take())
                        .unwrap_or_else(|| "GPU".into());
                    gpus.push(GpuInfo {
                        index: *index,
                        name: label,
                        api: "ROCm",
                    });
                    *index += 1;
                }
                *name = None;
                *marketing = None;
                *is_gpu = false;
            };
            for line in text.lines() {
                let t = line.trim();
                if t.starts_with("Agent ") && t.ends_with(':') || t.starts_with("*******") {
                    flush(
                        &mut gpus,
                        &mut name,
                        &mut marketing,
                        &mut is_gpu,
                        &mut index,
                    );
                } else if let Some(v) = t.strip_prefix("Marketing Name:") {
                    marketing = Some(v.trim().to_string());
                } else if let Some(v) = t.strip_prefix("Name:") {
                    if name.is_none() {
                        name = Some(v.trim().to_string());
                    }
                } else if let Some(v) = t.strip_prefix("Device Type:") {
                    is_gpu = v.trim() == "GPU";
                }
            }
            flush(
                &mut gpus,
                &mut name,
                &mut marketing,
                &mut is_gpu,
                &mut index,
            );
            gpus
        })
        .clone()
}

/// Resolve "auto" to the best available backend.
pub fn resolve(setting: &str) -> Backend {
    match parse_setting(setting) {
        Some(b) => b,
        None => {
            if gpu_runtime_available() && !gpus().is_empty() {
                Backend::Rocm {
                    device: gpus()[0].index,
                }
            } else {
                Backend::Cpu
            }
        }
    }
}
