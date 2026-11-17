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
    /// "ROCm", "CUDA" or "DirectML".
    pub api: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Cpu,
    Rocm {
        device: i32,
    },
    Cuda {
        device: i32,
    },
    /// Windows: any graphics card through DirectML.
    DirectMl {
        device: i32,
    },
}

impl Backend {
    pub fn label(self, gpus: &[GpuInfo]) -> String {
        match self {
            Backend::Cpu => "CPU".into(),
            Backend::Rocm { device } | Backend::Cuda { device } | Backend::DirectMl { device } => {
                gpus.iter()
                    .find(|g| g.index == device)
                    .map(|g| format!("GPU {} ({})", g.index, g.name))
                    .unwrap_or_else(|| format!("GPU {device} ({})", self.api()))
            }
        }
    }

    pub fn api(self) -> &'static str {
        match self {
            Backend::Cpu => "CPU",
            Backend::Rocm { .. } => "ROCm",
            Backend::Cuda { .. } => "CUDA",
            Backend::DirectMl { .. } => "DirectML",
        }
    }

    pub fn device(self) -> Option<i32> {
        match self {
            Backend::Cpu => None,
            Backend::Rocm { device } | Backend::Cuda { device } | Backend::DirectMl { device } => {
                Some(device)
            }
        }
    }
}

/// Which ONNX Runtime library a dynamic build loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeKind {
    Cpu,
    Rocm,
    Cuda,
    /// Microsoft's DirectML build (Windows); also runs on the processor.
    DirectMl,
}

/// The setting string stored in the parameters: "auto", "cpu", "rocm:N",
/// "cuda:N", "dml:N".
pub fn parse_setting(s: &str) -> Option<Backend> {
    match s.trim() {
        "" | "auto" => None,
        "cpu" => Some(Backend::Cpu),
        other => {
            if let Some(n) = other.strip_prefix("rocm:") {
                n.parse().ok().map(|device| Backend::Rocm { device })
            } else if let Some(n) = other.strip_prefix("cuda:") {
                n.parse().ok().map(|device| Backend::Cuda { device })
            } else if let Some(n) = other.strip_prefix("dml:") {
                n.parse().ok().map(|device| Backend::DirectMl { device })
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
        Some(Backend::DirectMl { device }) => format!("dml:{device}"),
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
/// `scripts/get-onnxruntime.sh --cuda` together with NVIDIA's libraries
/// (override with ORT_CUDA_DYLIB_PATH).
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
    // The runtime alone is not enough: the CUDA provider is a separate
    // library next to it (an install that lacks it can only ever run on the
    // CPU, so it does not count).
    dirs.iter()
        .filter(|d| d.join("libonnxruntime_providers_cuda.so").is_file())
        .find_map(|d| newest_lib(d))
}

/// The NVIDIA libraries Microsoft's CUDA build of ONNX Runtime links
/// against: the CUDA 12 runtime and cuDNN 9. In an order in which each can
/// be loaded on its own.
pub const CUDA_LIBS: [&str; 7] = [
    "libcudart.so.12",
    "libcublasLt.so.12",
    "libcublas.so.12",
    "libcufft.so.11",
    "libcurand.so.10",
    "libnvrtc.so.12",
    "libcudnn.so.9",
];

/// Which of [`CUDA_LIBS`] cannot be loaded. Cached.
///
/// `get-runtime --cuda` puts NVIDIA's libraries next to the CUDA build of
/// ONNX Runtime, a folder the dynamic loader does not search (the CUDA
/// provider has no RUNPATH). Each one is therefore loaded from there by its
/// full path first; once loaded, the provider's dependency on the same
/// name resolves to it. What is not in that folder is looked up on the
/// system. NVIDIA's libraries find each other (cuDNN's parts, the NVRTC
/// built-ins) through their own `$ORIGIN` RUNPATH.
pub fn missing_cuda_libs() -> Vec<&'static str> {
    static CACHE: OnceLock<Vec<&'static str>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            #[cfg(feature = "gpu")]
            {
                let beside = cuda_runtime_lib().and_then(|p| p.parent().map(|d| d.to_path_buf()));
                // SAFETY: these are NVIDIA's own runtime libraries; loading
                // one runs only its initialisers. Kept open (the provider
                // needs them anyway).
                let load = |name: &std::ffi::OsStr| match unsafe { libloading::Library::new(name) }
                {
                    Ok(lib) => {
                        std::mem::forget(lib);
                        true
                    }
                    Err(e) => {
                        log::debug!("{}: {e}", name.to_string_lossy());
                        false
                    }
                };
                CUDA_LIBS
                    .iter()
                    .copied()
                    .filter(|name| {
                        let local = beside
                            .as_ref()
                            .map(|d| d.join(name))
                            .filter(|p| p.is_file());
                        !(local.is_some_and(|p| load(p.as_os_str())) || load(name.as_ref()))
                    })
                    .collect()
            }
            #[cfg(not(feature = "gpu"))]
            {
                Vec::new()
            }
        })
        .clone()
}

/// What is missing for CUDA in a few words, `None` when everything is found.
pub fn missing_cuda_summary() -> Option<String> {
    let missing = missing_cuda_libs();
    if missing.is_empty() {
        None
    } else if missing.len() == CUDA_LIBS.len() {
        Some("the CUDA 12 and cuDNN 9 libraries are missing".into())
    } else if missing == ["libcudnn.so.9"] {
        Some("cuDNN 9 is missing (libcudnn.so.9)".into())
    } else {
        Some(format!("{} not found", missing.join(", ")))
    }
}

static GPU_FAILED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// The GPU was tried and could not be used (the provider failed to load or
/// to register). From here on everything resolves to the CPU, so the status
/// line and the thread count match what is really running.
pub fn mark_gpu_failed(reason: &str) {
    let mut slot = GPU_FAILED.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        log::warn!("the GPU cannot be used ({reason}); the AI runs on the CPU until restart");
        *slot = Some(reason.to_string());
    }
}

/// Why the GPU that is present is not in use, in words for the user.
/// `None` when the GPU works or there is none to use.
pub fn gpu_failure() -> Option<String> {
    if runtime_kind() == RuntimeKind::Cuda
        && let Some(missing) = missing_cuda_summary()
    {
        return Some(format!(
            "The NVIDIA GPU cannot be used yet: {missing}. Run `voice-changer get-runtime --cuda` to download them, then restart. The AI runs on the CPU meanwhile."
        ));
    }
    GPU_FAILED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|reason| {
            format!(
                "The GPU could not be used ({reason}). The AI runs on the CPU until you restart."
            )
        })
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
/// The runtime library in `dir`, if there is one.
#[cfg(windows)]
fn newest_lib(dir: &std::path::Path) -> Option<PathBuf> {
    let p = dir.join("onnxruntime.dll");
    p.is_file().then_some(p)
}

#[cfg(not(windows))]
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
    #[cfg(windows)]
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("voice-changer"));
    }
    #[cfg(not(windows))]
    {
        dirs.push("/usr/lib64/voice-changer".into());
        dirs.push("/usr/lib/voice-changer".into());
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join(".local/lib/voice-changer"));
        }
        dirs.push("/usr/lib64".into());
        dirs.push("/usr/lib/x86_64-linux-gnu".into());
        dirs.push("/usr/lib".into());
    }
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
                // Windows: the DirectML build has its helper library beside it.
                let directml =
                    cfg!(windows) && p.parent().is_some_and(|d| d.join("DirectML.dll").is_file());
                return Some((
                    p,
                    if directml {
                        RuntimeKind::DirectMl
                    } else {
                        RuntimeKind::Cpu
                    },
                ));
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
        && runtime_kind() == RuntimeKind::Cpu
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
        Some((p, RuntimeKind::DirectMl)) => {
            format!("ONNX Runtime with DirectML: {}", p.display())
        }
        Some((p, RuntimeKind::Cpu)) => format!("ONNX Runtime (CPU): {}", p.display()),
        None if cfg!(windows) => {
            "ONNX Runtime not found: onnxruntime.dll must sit next to voice-changer.exe".to_string()
        }
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
    if let Some(why) = gpu_failure() {
        return Some(why);
    }
    if runtime_kind() == RuntimeKind::DirectMl {
        return Some("The graphics card (DirectML) is experimental on Windows: Auto uses the processor. Pick the graphics card to try it; if it cannot run the voice, the processor takes over.".into());
    }
    if !gpus().is_empty() {
        return None;
    }
    if cfg!(windows) {
        return Some("The AI runs on the processor.".into());
    }
    if !cfg!(feature = "gpu") {
        return Some("GPU support needs the gpu build.".into());
    }
    if !nvidia_gpus().is_empty() {
        return Some("NVIDIA GPU found. Run `voice-changer get-runtime --cuda` (a 1.7 GB download), then restart.".into());
    }
    if rocm_runtime_lib().is_some() {
        return Some("No AMD GPU detected by ROCm; the AI runs on the CPU runtime.".into());
    }
    Some("AMD: install onnxruntime-rocm. NVIDIA: run `voice-changer get-runtime --cuda`.".into())
}

/// GPUs the loaded runtime can use: NVIDIA via `nvidia-smi` with the CUDA
/// runtime, AMD via `rocminfo` with the ROCm runtime. Cached.
pub fn gpus() -> Vec<GpuInfo> {
    GPUS.get_or_init(|| match runtime_kind() {
        RuntimeKind::Cuda => nvidia_gpus(),
        RuntimeKind::Cpu => Vec::new(),
        RuntimeKind::Rocm => rocm_gpus(),
        RuntimeKind::DirectMl => vec![GpuInfo {
            index: 0,
            name: "Graphics card".into(),
            api: "DirectML",
        }],
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

/// Resolve the setting to the backend that will really be used: "auto"
/// becomes the best available one, and a GPU that turned out not to work
/// becomes the CPU.
pub fn resolve(setting: &str) -> Backend {
    if gpu_failure().is_some() {
        return Backend::Cpu;
    }
    // DirectML is opt-in: Auto never picks it (see `gpu_note`).
    if runtime_kind() == RuntimeKind::DirectMl && parse_setting(setting).is_none() {
        return Backend::Cpu;
    }
    match parse_setting(setting) {
        Some(b) => b,
        None => match (runtime_kind(), gpus().first()) {
            (RuntimeKind::Cuda, Some(g)) => Backend::Cuda { device: g.index },
            (RuntimeKind::Rocm, Some(g)) => Backend::Rocm { device: g.index },
            _ => Backend::Cpu,
        },
    }
}

/// DirectML runs inside the graphics driver, where a failure can take the
/// whole program down instead of returning an error. A marker file is
/// written before the first voice is loaded on it and removed once that
/// worked; finding the marker at the next start means the last attempt
/// crashed, and the graphics card is left alone from then on.
fn directml_marker() -> Option<PathBuf> {
    crate::presets::config_dir().map(|d| d.join("directml-trying"))
}

/// Call before loading models on DirectML. Returns false when the last
/// attempt never finished (it crashed): the caller must use the processor.
pub fn directml_begin() -> bool {
    let Some(marker) = directml_marker() else {
        return true;
    };
    if marker.exists() {
        mark_gpu_failed("the graphics card crashed the AI last time");
        return false;
    }
    if let Some(dir) = marker.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&marker, b"");
    true
}

/// The models loaded and ran: the graphics card is fine.
pub fn directml_ok() {
    if let Some(marker) = directml_marker() {
        let _ = std::fs::remove_file(marker);
    }
}

/// Let the user try the graphics card again after a crash (picking it in
/// the Compute list calls this).
pub fn directml_forget_crash() {
    directml_ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_strings_round_trip() {
        for b in [
            None,
            Some(Backend::Cpu),
            Some(Backend::Rocm { device: 1 }),
            Some(Backend::Cuda { device: 0 }),
            Some(Backend::DirectMl { device: 0 }),
        ] {
            assert_eq!(parse_setting(&setting_string(b)), b);
        }
        assert_eq!(parse_setting("nonsense"), None);
    }
}
