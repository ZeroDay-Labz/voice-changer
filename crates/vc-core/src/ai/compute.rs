//! Where the AI models run. Detected at startup; the user picks Auto, CPU
//! or a specific GPU and the worker reloads the models accordingly.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuInfo {
    pub index: i32,
    pub name: String,
    /// "ROCm" or "CUDA".
    pub api: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Cpu,
    Rocm { device: i32 },
    Cuda { device: i32 },
}

impl Backend {
    pub fn label(self, gpus: &[GpuInfo]) -> String {
        match self {
            Backend::Cpu => "CPU".into(),
            Backend::Rocm { device } | Backend::Cuda { device } => gpus
                .iter()
                .find(|g| g.index == device)
                .map(|g| format!("GPU {} ({})", g.index, g.name))
                .unwrap_or_else(|| format!("GPU {device} ({})", self.api())),
        }
    }

    pub fn api(self) -> &'static str {
        match self {
            Backend::Cpu => "CPU",
            Backend::Rocm { .. } => "ROCm",
            Backend::Cuda { .. } => "CUDA",
        }
    }

    pub fn device(self) -> Option<i32> {
        match self {
            Backend::Cpu => None,
            Backend::Rocm { device } | Backend::Cuda { device } => Some(device),
        }
    }
}

/// Which ONNX Runtime library a dynamic build loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeKind {
    Cpu,
    Rocm,
    Cuda,
}

/// The setting string stored in the parameters: "auto", "cpu", "rocm:N", "cuda:N".
pub fn parse_setting(s: &str) -> Option<Backend> {
    match s.trim() {
        "" | "auto" => None,
        "cpu" => Some(Backend::Cpu),
        other => {
            if let Some(n) = other.strip_prefix("rocm:") {
                n.parse().ok().map(|device| Backend::Rocm { device })
            } else if let Some(n) = other.strip_prefix("cuda:") {
                n.parse().ok().map(|device| Backend::Cuda { device })
            } else {
                None
            }
        }
    }
}

pub fn setting_string(b: Option<Backend>) -> String {
    match b {
        None => "auto".into(),
        Some(Backend::Cpu) => "cpu".into(),
        Some(Backend::Rocm { device }) => format!("rocm:{device}"),
        Some(Backend::Cuda { device }) => format!("cuda:{device}"),
    }
}

/// NVIDIA GPUs as reported by `nvidia-smi` (present whenever the driver is). Cached.
pub fn nvidia_gpus() -> Vec<GpuInfo> {
    static CACHE: std::sync::OnceLock<Vec<GpuInfo>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            let Ok(out) = std::process::Command::new("nvidia-smi")
                .args(["--query-gpu=name", "--format=csv,noheader"])
                .output()
            else {
                return Vec::new();
            };
            if !out.status.success() {
                return Vec::new();
            }
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .enumerate()
                .map(|(i, name)| GpuInfo {
                    index: i as i32,
                    name: name.to_string(),
                    api: "CUDA",
                })
                .collect()
        })
        .clone()
}

/// Microsoft's CUDA build of ONNX Runtime, installed by
/// `scripts/get-onnxruntime.sh --cuda` (override with ORT_CUDA_DYLIB_PATH).
pub fn cuda_runtime_lib() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ORT_CUDA_DYLIB_PATH") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    {
        dirs.push(exe_dir.join("cuda"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/lib/voice-changer/cuda"));
    }
    dirs.push("/usr/lib64/voice-changer/cuda".into());
    dirs.push("/usr/lib/voice-changer/cuda".into());
    dirs.iter().find_map(|d| newest_lib(d))
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

/// Whether this build can use a GPU at all (a ROCm or CUDA runtime is loaded).
pub fn gpu_runtime_available() -> bool {
    cfg!(feature = "gpu") && runtime_kind() != RuntimeKind::Cpu
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

/// The runtime library a dynamic build will load: CUDA when an NVIDIA card
/// and the CUDA runtime are both present, else ROCm, else CPU. Cached.
pub fn runtime_lib() -> Option<(PathBuf, RuntimeKind)> {
    static CACHE: std::sync::OnceLock<Option<(PathBuf, RuntimeKind)>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            if !nvidia_gpus().is_empty()
                && let Some(p) = cuda_runtime_lib()
            {
                return Some((p, RuntimeKind::Cuda));
            }
            if let Some(p) = rocm_runtime_lib() {
                return Some((p, RuntimeKind::Rocm));
            }
            cpu_runtime_lib().map(|p| (p, RuntimeKind::Cpu))
        })
        .clone()
}

pub fn runtime_kind() -> RuntimeKind {
    runtime_lib().map(|(_, k)| k).unwrap_or(RuntimeKind::Cpu)
}

/// One line for the Settings page: which ONNX Runtime is in use.
pub fn runtime_description() -> String {
    if !cfg!(feature = "gpu") {
        return "ONNX Runtime built in (CPU)".to_string();
    }
    match runtime_lib() {
        Some((p, RuntimeKind::Cuda)) => format!("ONNX Runtime with CUDA: {}", p.display()),
        Some((p, RuntimeKind::Rocm)) => format!("ONNX Runtime with ROCm: {}", p.display()),
        Some((p, RuntimeKind::Cpu)) => format!("ONNX Runtime (CPU): {}", p.display()),
        None => "ONNX Runtime not found: install onnxruntime or onnxruntime-rocm".to_string(),
    }
}

/// Why no GPU is offered, for the Compute picker. `None` when GPUs are listed.
pub fn gpu_note() -> Option<String> {
    if !gpus().is_empty() {
        return None;
    }
    if !cfg!(feature = "gpu") {
        return Some("GPU support needs the gpu build.".into());
    }
    if !nvidia_gpus().is_empty() {
        return Some("NVIDIA GPU found. Run scripts/get-onnxruntime.sh --cuda (needs the CUDA 12 runtime and cuDNN 9), then restart.".into());
    }
    if rocm_runtime_lib().is_some() {
        return Some("No GPU detected by ROCm.".into());
    }
    Some("AMD: install onnxruntime-rocm. NVIDIA: run scripts/get-onnxruntime.sh --cuda.".into())
}

/// GPUs the loaded runtime can use: NVIDIA via `nvidia-smi` with the CUDA
/// runtime, AMD via `rocminfo` with the ROCm runtime. Cached.
pub fn gpus() -> Vec<GpuInfo> {
    static CACHE: std::sync::OnceLock<Vec<GpuInfo>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            match runtime_kind() {
                RuntimeKind::Cuda => return nvidia_gpus(),
                RuntimeKind::Cpu => return Vec::new(),
                RuntimeKind::Rocm => {}
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
        None => match (runtime_kind(), gpus().first()) {
            (RuntimeKind::Cuda, Some(g)) => Backend::Cuda { device: g.index },
            (RuntimeKind::Rocm, Some(g)) => Backend::Rocm { device: g.index },
            _ => Backend::Cpu,
        },
    }
}
