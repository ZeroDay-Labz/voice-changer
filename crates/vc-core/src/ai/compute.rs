//! Where the AI models run. Detected at startup; the user picks Auto, CPU
//! or a specific GPU and the worker reloads the models accordingly.

use std::path::PathBuf;
use std::sync::OnceLock;

static NVIDIA: OnceLock<Vec<GpuInfo>> = OnceLock::new();
static RUNTIME: OnceLock<Option<(PathBuf, RuntimeKind)>> = OnceLock::new();
static GPUS: OnceLock<Vec<GpuInfo>> = OnceLock::new();

/// Run the slow probes (`nvidia-smi`, `rocminfo`, library search) on a
/// background thread so the window never waits for them. Call once at
/// startup; everything else reads the cached results.
pub fn probe_in_background() {
    if GPUS.get().is_some() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("gpu-probe".into())
        .spawn(|| {
            let _ = runtime_lib();
            let _ = gpus();
            let _ = super::hardware::info();
        });
}

/// The GPU list if the probe has finished, without blocking on it.
pub fn gpus_if_probed() -> Option<Vec<GpuInfo>> {
    GPUS.get().cloned()
}

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
    NVIDIA
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
    candidates
        .iter()
        .find_map(|dir| newest_lib(std::path::Path::new(dir)))
}

/// Whether this build can use a GPU at all (a ROCm or CUDA runtime is loaded).
pub fn gpu_runtime_available() -> bool {
    cfg!(feature = "gpu") && runtime_kind() != RuntimeKind::Cpu
}

/// The highest-versioned `libonnxruntime.so*` in `dir`, comparing version
/// numbers numerically (a plain string sort puts `so.1.9` after `so.1.22`).
/// Libraries older than the API this build was compiled against are skipped.
fn newest_lib(dir: &std::path::Path) -> Option<PathBuf> {
    let mut libs: Vec<(Vec<u32>, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .map(|f| f.to_string_lossy().starts_with("libonnxruntime.so"))
                .unwrap_or(false)
        })
        .map(|p| (lib_version(&p), p))
        .filter(|(v, _)| v.is_empty() || v >= &MIN_RUNTIME_VERSION.to_vec())
        .collect();
    // A bare `libonnxruntime.so` symlink says nothing about its version:
    // prefer any explicitly versioned file, then the highest version.
    libs.sort_by(|a, b| a.0.cmp(&b.0));
    libs.pop().map(|(_, p)| p)
}

/// Oldest ONNX Runtime this build can load (the C API version it targets).
pub const MIN_RUNTIME_VERSION: [u32; 2] = [1, 22];

/// `libonnxruntime.so.1.22.0` → `[1, 22, 0]`; a bare `.so` → `[]`.
pub fn lib_version(path: &std::path::Path) -> Vec<u32> {
    let name = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    name.strip_prefix("libonnxruntime.so.")
        .map(|v| v.split('.').filter_map(|n| n.parse().ok()).collect())
        .unwrap_or_default()
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
    RUNTIME
        .get_or_init(|| {
            if !nvidia_gpus().is_empty()
                && let Some(p) = cuda_runtime_lib()
            {
                return Some((p, RuntimeKind::Cuda));
            }
            // The ROCm build of ONNX Runtime aborts (a C++ assertion on an
            // empty device list) when it is loaded on a machine without a
            // ROCm GPU, so it is only used when `rocminfo` reports one. An
            // explicit ORT_DYLIB_PATH is honoured regardless.
            let rocm = rocm_runtime_lib();
            let forced = std::env::var_os("ORT_DYLIB_PATH").is_some();
            if let Some(p) = rocm.clone()
                && (forced || !rocm_gpus().is_empty())
            {
                return Some((p, RuntimeKind::Rocm));
            }
            if let Some(p) = cpu_runtime_lib() {
                return Some((p, RuntimeKind::Cpu));
            }
            // No CPU library anywhere: the ROCm one is the only option left.
            rocm.map(|p| (p, RuntimeKind::Rocm))
        })
        .clone()
}

/// Why the installed ROCm runtime is not in use, for the Settings page and
/// the doctor. `None` when there is nothing to explain.
pub fn rocm_note() -> Option<String> {
    if cfg!(feature = "gpu")
        && rocm_runtime_lib().is_some()
        && runtime_kind() != RuntimeKind::Rocm
        && std::env::var_os("ORT_DYLIB_PATH").is_none()
    {
        Some("onnxruntime-rocm is installed but rocminfo reports no AMD GPU, so the CPU runtime is used (the ROCm one would crash without a GPU).".into())
    } else {
        None
    }
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

/// `runtime_description` plus, when relevant, why the ROCm runtime is idle.
pub fn runtime_description_full() -> String {
    match rocm_note() {
        Some(note) => format!("{} · {note}", runtime_description()),
        None => runtime_description(),
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
        return Some("No AMD GPU detected by ROCm; the AI runs on the CPU runtime.".into());
    }
    Some("AMD: install onnxruntime-rocm. NVIDIA: run scripts/get-onnxruntime.sh --cuda.".into())
}

/// GPUs the loaded runtime can use: NVIDIA via `nvidia-smi` with the CUDA
/// runtime, AMD via `rocminfo` with the ROCm runtime. Cached.
pub fn gpus() -> Vec<GpuInfo> {
    GPUS.get_or_init(|| match runtime_kind() {
        RuntimeKind::Cuda => nvidia_gpus(),
        RuntimeKind::Cpu => Vec::new(),
        RuntimeKind::Rocm => rocm_gpus(),
    })
    .clone()
}

/// AMD GPUs as reported by `rocminfo` (independent of which runtime is loaded). Cached.
pub fn rocm_gpus() -> Vec<GpuInfo> {
    static CACHE: OnceLock<Vec<GpuInfo>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
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
