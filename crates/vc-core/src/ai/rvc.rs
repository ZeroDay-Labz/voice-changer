//! The three-model RVC inference chain on ONNX Runtime.

use super::f0;
use super::mel::{MelSpectrogram, N_MELS};
use anyhow::{Context as _, Result, anyhow, bail};
use ort::session::Session;
use ort::value::Tensor;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct ModelPaths {
    pub contentvec: PathBuf,
    pub rmvpe: PathBuf,
    pub voice: PathBuf,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Timings {
    pub features_ms: f32,
    pub pitch_ms: f32,
    pub synth_ms: f32,
}

impl Timings {
    pub fn total_ms(&self) -> f32 {
        self.features_ms + self.pitch_ms + self.synth_ms
    }
}

/// Tiny xorshift + Box-Muller: the synthesizer wants N(0,1) noise and we
/// don't need a crypto RNG for that.
struct Gauss(u64);

impl Gauss {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn uniform(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn fill(&mut self, out: &mut [f32]) {
        for pair in out.chunks_mut(2) {
            let u1 = self.uniform().max(1e-7);
            let u2 = self.uniform();
            let r = (-2.0 * u1.ln()).sqrt();
            let (s, c) = (2.0 * std::f32::consts::PI * u2).sin_cos();
            pair[0] = r * c;
            if pair.len() > 1 {
                pair[1] = r * s;
            }
        }
    }
}

/// The base models (ContentVec + RMVPE) do not change between voices, so
/// their sessions are kept and shared across loads: switching voices then
/// only opens the voice model, and the GPU kernels for the base models stay
/// warm. Keyed by paths, backend and thread count.
struct BaseSessions {
    key: (PathBuf, PathBuf, super::compute::Backend, usize),
    contentvec: Arc<Mutex<Session>>,
    rmvpe: Arc<Mutex<Session>>,
}

static BASE_SESSIONS: Mutex<Option<BaseSessions>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct Rvc {
    contentvec: Arc<Mutex<Session>>,
    rmvpe: Arc<Mutex<Session>>,
    voice: Session,
    mel: MelSpectrogram,
    rng: Gauss,
    /// Output sample rate of the voice model, detected on first inference.
    model_rate: Option<usize>,
    /// Retrieval index for this voice, if it shipped one.
    pub index: Option<super::index::RetrievalIndex>,
    /// 0..1 blend toward the index (RVC `index_rate`); 0 disables.
    pub index_rate: f32,
    // scratch
    mel_buf: Vec<f32>,
    feats: Vec<f32>,
    salience: Vec<f32>,
    f0: Vec<f32>,
    phone: Vec<f32>,
    rnd: Vec<f32>,
    pub last_f0_hz: f32,
}

/// Name of the accelerator this build defaults to, for the UI.
pub fn backend_name() -> &'static str {
    if super::compute::gpu_runtime_available() {
        "ROCm GPU"
    } else {
        "CPU"
    }
}

/// ONNX Runtime looks for `libonnxruntime_providers_shared.so` and the
/// ROCm provider by their *unversioned* names next to its own library,
/// but Fedora only installs versioned files. Build a directory of symlinks
/// with the expected names and load the runtime from there.
#[cfg(feature = "gpu-rocm")]
fn rocm_runtime_path() -> Option<std::path::PathBuf> {
    // No ROCm runtime installed: use a CPU library (next to the executable,
    // the package's lib dir, or the distribution's), so one binary serves
    // machines with and without a GPU.
    let Some(rocm_lib) = super::compute::rocm_runtime_lib() else {
        return super::compute::cpu_runtime_lib();
    };
    let system = rocm_lib
        .parent()
        .unwrap_or(std::path::Path::new("/usr/lib64/rocm/lib"));
    let Some(dir) = super::models_dir().and_then(|m| m.parent().map(|p| p.join("ort-rocm"))) else {
        return Some(rocm_lib);
    };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(entries) = std::fs::read_dir(system) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            // libfoo.so.1.22.2 -> libfoo.so
            if let Some(idx) = name.find(".so") {
                let base = format!("{}.so", &name[..idx]);
                let link = dir.join(&base);
                if !link.exists() {
                    let _ = std::os::unix::fs::symlink(e.path(), &link);
                }
            }
        }
    }
    Some(dir.join("libonnxruntime.so"))
}

#[cfg(feature = "gpu-rocm")]
static RUNTIME_OK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Load the dynamic runtime once; `Ok` when sessions can be created.
#[cfg(feature = "gpu-rocm")]
fn init_runtime() -> Result<()> {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Some(path) = rocm_runtime_path() else {
            log::error!("no ONNX Runtime library found (install onnxruntime or onnxruntime-rocm, or set ORT_DYLIB_PATH)");
            return;
        };
        // MIOpen prints kernel-search chatter straight to stderr; keep errors only.
        if std::env::var_os("MIOPEN_LOG_LEVEL").is_none() {
            // SAFETY: called once during startup before the runtime (and its
            // threads) are loaded; nothing else reads the environment concurrently.
            unsafe { std::env::set_var("MIOPEN_LOG_LEVEL", "3") };
        }
        let path = path.to_string_lossy().to_string();
        // The ROCm provider library resolves abseil & co. from the global
        // symbol scope, but ort opens the runtime RTLD_LOCAL. Open it
        // RTLD_GLOBAL first (and keep it open) so the provider can link.
        unsafe {
            use libloading::os::unix::{Library, RTLD_GLOBAL, RTLD_NOW};
            match Library::open(Some(&path), RTLD_NOW | RTLD_GLOBAL) {
                Ok(lib) => std::mem::forget(lib),
                Err(e) => log::warn!("could not preload {path}: {e}"),
            }
        }
        match ort::init_from(&path) {
            Ok(builder) => {
                builder.commit();
                RUNTIME_OK.store(true, std::sync::atomic::Ordering::Relaxed);
                log::info!("ONNX Runtime loaded from {path}");
            }
            Err(e) => log::error!("could not load ONNX Runtime from {path}: {e} (install onnxruntime-rocm or set ORT_DYLIB_PATH)"),
        }
    });
    if RUNTIME_OK.load(std::sync::atomic::Ordering::Relaxed) {
        Ok(())
    } else {
        Err(anyhow!(
            "ONNX Runtime not found. Install the onnxruntime (CPU) or onnxruntime-rocm (AMD GPU) package, or set ORT_DYLIB_PATH."
        ))
    }
}

/// Register the ROCm execution provider through the classic C API entry
/// point. The ort crate's own registration goes through ONNX Runtime's
/// newer plugin-device API, which the 1.22 ROCm build aborts on.
#[cfg(feature = "gpu-rocm")]
fn append_rocm(builder: &ort::session::builder::SessionBuilder, device: i32) -> Result<()> {
    use ort::AsPointer;
    use ort::sys;
    let api = ort::api();
    unsafe fn check(api: &sys::OrtApi, status: sys::OrtStatusPtr) -> Result<()> {
        let raw = status.0;
        if raw.is_null() {
            return Ok(());
        }
        // SAFETY: a non-null status is a valid OrtStatus owned by us.
        let msg = unsafe { std::ffi::CStr::from_ptr((api.GetErrorMessage)(raw)) }
            .to_string_lossy()
            .into_owned();
        unsafe { (api.ReleaseStatus)(raw) };
        Err(anyhow!("ROCm execution provider: {msg}"))
    }
    // SAFETY: plain C API calls on a live session-options object; the
    // provider options are created and released here.
    unsafe {
        let mut opts: *mut sys::OrtROCMProviderOptions = std::ptr::null_mut();
        check(api, (api.CreateROCMProviderOptions)(&mut opts))?;
        let device_s = std::ffi::CString::new(device.to_string()).unwrap_or_default();
        {
            let k = [c"device_id".as_ptr()];
            let v = [device_s.as_ptr()];
            check(
                api,
                (api.UpdateROCMProviderOptions)(opts, k.as_ptr(), v.as_ptr(), 1),
            )?;
        }
        // Let MIOpen search for the fastest conv kernels (cached on disk) and
        // enable TunableOp for GEMMs; set VC_ROCM_TUNE=0 to skip.
        if std::env::var("VC_ROCM_TUNE")
            .map(|v| v != "0")
            .unwrap_or(true)
        {
            let keys = [
                c"miopen_conv_exhaustive_search",
                c"miopen_conv_use_max_workspace",
                c"tunable_op_enable",
                c"tunable_op_tuning_enable",
            ];
            let vals = [c"1", c"1", c"1", c"1"];
            let k: Vec<*const std::ffi::c_char> = keys.iter().map(|c| c.as_ptr()).collect();
            let v: Vec<*const std::ffi::c_char> = vals.iter().map(|c| c.as_ptr()).collect();
            if let Err(e) = check(
                api,
                (api.UpdateROCMProviderOptions)(opts, k.as_ptr(), v.as_ptr(), k.len()),
            ) {
                log::warn!("{e} (continuing with default ROCm options)");
            }
        }
        let status = (api.SessionOptionsAppendExecutionProvider_ROCM)(
            builder.ptr() as *mut sys::OrtSessionOptions,
            opts,
        );
        (api.ReleaseROCMProviderOptions)(opts);
        check(api, status)
    }
}

impl Rvc {
    pub fn load(paths: &ModelPaths, threads: usize) -> Result<Self> {
        Self::load_on(paths, threads, super::compute::Backend::Cpu)
    }

    /// Load the three models on the given backend. GPU requests on a build
    /// or machine without the ROCm runtime fall back to the CPU.
    pub fn load_on(
        paths: &ModelPaths,
        threads: usize,
        backend: super::compute::Backend,
    ) -> Result<Self> {
        #[cfg(feature = "gpu-rocm")]
        init_runtime()?;
        #[cfg(not(feature = "gpu-rocm"))]
        let _ = backend;
        let make_builder = || -> Result<ort::session::builder::SessionBuilder> {
            Session::builder()
                .map_err(|e| anyhow!("{e}"))?
                .with_intra_threads(threads)
                .map_err(|e| anyhow!("{e}"))?
                .with_optimization_level(ort::session::builder::GraphOptimizationLevel::All)
                .map_err(|e| anyhow!("{e}"))
        };
        let open = |p: &PathBuf| -> Result<Session> {
            let mut builder = make_builder()?;
            #[cfg(feature = "gpu-rocm")]
            if let super::compute::Backend::Rocm { device } = backend
                && super::compute::gpu_runtime_available()
                && let Err(e) = append_rocm(&builder, device)
            {
                // A failed registration leaves the options object unusable
                // (ONNX Runtime 1.22 aborts on it), so start over on the CPU.
                log::warn!("{e}; this session will run on the CPU");
                builder = make_builder()?;
            }
            builder
                .commit_from_file(p)
                .with_context(|| format!("loading {}", p.display()))
        };
        let t0 = Instant::now();
        let key = (
            paths.contentvec.clone(),
            paths.rmvpe.clone(),
            backend,
            threads,
        );
        let (contentvec, rmvpe) = {
            let mut cache = lock(&BASE_SESSIONS);
            match cache.as_ref() {
                Some(b) if b.key == key => (b.contentvec.clone(), b.rmvpe.clone()),
                _ => {
                    let contentvec = Arc::new(Mutex::new(open(&paths.contentvec)?));
                    let rmvpe = Arc::new(Mutex::new(open(&paths.rmvpe)?));
                    let old = cache.replace(BaseSessions {
                        key,
                        contentvec: contentvec.clone(),
                        rmvpe: rmvpe.clone(),
                    });
                    // ROCm sessions must not be torn down (see `stream::discard`).
                    if cfg!(feature = "gpu-rocm") {
                        std::mem::forget(old);
                    }
                    log::info!(
                        "base models loaded in {:.0} ms",
                        t0.elapsed().as_secs_f32() * 1000.0
                    );
                    (contentvec, rmvpe)
                }
            }
        };
        let t1 = Instant::now();
        let voice = open(&paths.voice)?;
        log::info!(
            "voice model loaded in {:.0} ms",
            t1.elapsed().as_secs_f32() * 1000.0
        );

        // Sanity-check the voice model's interface so a wrong file fails loudly.
        let names: Vec<&str> = voice.inputs().iter().map(|o| o.name()).collect();
        for required in ["phone", "phone_lengths", "pitch", "pitchf", "ds", "rnd"] {
            if !names.contains(&required) {
                bail!(
                    "{} is not an RVC voice model (missing input '{required}'; has {names:?})",
                    paths.voice.display()
                );
            }
        }
        let rvc = Self {
            contentvec,
            rmvpe,
            voice,
            mel: MelSpectrogram::new(),
            rng: Gauss(0x9E37_79B9_7F4A_7C15),
            model_rate: None,
            index: None,
            index_rate: 0.0,
            mel_buf: Vec::new(),
            feats: Vec::new(),
            salience: Vec::new(),
            f0: Vec::new(),
            phone: Vec::new(),
            rnd: Vec::new(),
            last_f0_hz: 0.0,
        };
        Ok(rvc)
    }

    /// Run one inference on quiet noise with the given window/tail sizes.
    /// GPU providers compile kernels per tensor shape on first use (seconds
    /// to tens of seconds on ROCm), so streaming code warms up with its
    /// exact window before the first real block.
    pub fn warm_up(&mut self, window16k: usize, tail16k: usize) {
        let t0 = Instant::now();
        let mut rng = Gauss(0xD1B5_4A32_D192_ED03);
        let mut warm = vec![0.0f32; window16k.max(2048)];
        rng.fill(&mut warm);
        for v in warm.iter_mut() {
            *v *= 0.01;
        }
        let mut out = Vec::new();
        match self.infer_tail(&warm, tail16k, 0.0, 0, &mut out) {
            Err(e) => log::warn!("warm-up inference failed: {e:#}"),
            Ok(_) => log::info!(
                "models warmed up for a {:.2} s window in {:.0} ms",
                window16k as f32 / 16_000.0,
                t0.elapsed().as_secs_f32() * 1000.0
            ),
        }
    }

    pub fn model_rate(&self) -> Option<usize> {
        self.model_rate
    }

    /// Convert `audio16k` (mono, 16 kHz, -1..1) into the target voice.
    /// `out` receives audio at [`Self::model_rate`].
    pub fn infer(
        &mut self,
        audio16k: &[f32],
        semitones: f32,
        speaker: i64,
        out: &mut Vec<f32>,
    ) -> Result<Timings> {
        self.infer_tail(audio16k, audio16k.len(), semitones, speaker, out)
    }

    /// Like [`Self::infer`], but pitch detection and synthesis only cover
    /// the last `tail16k` samples (content features still see the whole
    /// window for context). The output covers that tail only. This is what
    /// makes realtime affordable: the synthesizer and pitch model are
    /// convolutional and only need a little context, while the content
    /// encoder benefits from more.
    pub fn infer_tail(
        &mut self,
        audio16k: &[f32],
        tail16k: usize,
        semitones: f32,
        speaker: i64,
        out: &mut Vec<f32>,
    ) -> Result<Timings> {
        self.infer_tail_with(audio16k, tail16k, semitones, speaker, 1.0, out)
    }

    /// `noise_scale` scales the synthesizer's random excitation (1.0 = the
    /// model's training condition; lower is cleaner/less breathy).
    #[allow(clippy::too_many_arguments)]
    pub fn infer_tail_with(
        &mut self,
        audio16k: &[f32],
        tail16k: usize,
        semitones: f32,
        speaker: i64,
        noise_scale: f32,
        out: &mut Vec<f32>,
    ) -> Result<Timings> {
        let mut timings = Timings::default();
        let n = audio16k.len();
        if n < 2048 {
            bail!("need at least 2048 samples of 16 kHz audio");
        }
        // Keep the tail on the 320-sample feature grid so frames line up.
        let tail16k = (tail16k.min(n) / 320) * 320;
        let tail_start = n - tail16k;
        let tail = &audio16k[tail_start..];

        // ---- content features -------------------------------------------------
        let t0 = Instant::now();
        let n_frames = {
            let input = Tensor::from_array((vec![1i64, n as i64], audio16k.to_vec()))?;
            let mask = Tensor::from_array((vec![1i64, n as i64], vec![1i64; n]))?;
            let mut session = lock(&self.contentvec);
            let outputs =
                session.run(ort::inputs!["input_values" => input, "attention_mask" => mask])?;
            let (shape, data) = outputs["hidden_states"].try_extract_tensor::<f32>()?;
            let frames = shape[1] as usize;
            let dim = shape[2] as usize;
            if dim != 768 {
                bail!("content encoder produced {dim}-d features, expected 768");
            }
            self.feats.clear();
            self.feats.extend_from_slice(data);
            frames
        };
        timings.features_ms = t0.elapsed().as_secs_f32() * 1000.0;

        // ---- pitch (tail only) ------------------------------------------------
        let t0 = Instant::now();
        let mel_frames = self.mel.compute(tail, &mut self.mel_buf);
        let padded = mel_frames.div_ceil(32) * 32;
        if padded != mel_frames {
            // [128][frames] → [128][padded] with zeros on the right.
            let mut p = vec![0.0f32; N_MELS * padded];
            for m in 0..N_MELS {
                p[m * padded..m * padded + mel_frames]
                    .copy_from_slice(&self.mel_buf[m * mel_frames..(m + 1) * mel_frames]);
            }
            self.mel_buf = p;
        }
        {
            let mel = Tensor::from_array((
                vec![1i64, N_MELS as i64, padded as i64],
                std::mem::take(&mut self.mel_buf),
            ))?;
            let mut session = lock(&self.rmvpe);
            let outputs = session.run(ort::inputs!["input" => mel])?;
            let (shape, data) = outputs["output"].try_extract_tensor::<f32>()?;
            let classes = shape[2] as usize;
            if classes != f0::N_CLASSES {
                bail!("pitch model produced {classes} classes, expected 360");
            }
            self.salience.clear();
            self.salience
                .extend_from_slice(&data[..mel_frames * classes]);
        }
        f0::decode(&self.salience, mel_frames, 0.03, &mut self.f0);
        f0::median3(&mut self.f0);
        f0::shift(&mut self.f0, semitones);
        self.last_f0_hz = {
            let voiced: Vec<f32> = self.f0.iter().copied().filter(|v| *v > 0.0).collect();
            if voiced.is_empty() {
                0.0
            } else {
                voiced.iter().sum::<f32>() / voiced.len() as f32
            }
        };
        timings.pitch_ms = t0.elapsed().as_secs_f32() * 1000.0;

        // ---- retrieval (tail frames only) --------------------------------------
        // Features are 50 fps (one per 320 samples); RVC repeats each frame
        // twice to reach the 100 fps pitch grid. Pick the frames covering the tail.
        let tail_frames = (tail16k / 320).min(n_frames);
        let first_frame = n_frames - tail_frames;
        if let Some(index) = &self.index
            && self.index_rate > 0.0
            && index.d == 768
        {
            let t_idx = Instant::now();
            let slice = &mut self.feats[first_frame * 768..n_frames * 768];
            index.blend(slice, self.index_rate, 1, 8);
            timings.pitch_ms += t_idx.elapsed().as_secs_f32() * 1000.0;
        }

        // ---- synthesis (tail only) ----------------------------------------------
        let t0 = Instant::now();
        let t = (tail_frames * 2).min(self.f0.len());
        if t == 0 {
            bail!("no frames to synthesize");
        }
        self.phone.clear();
        self.phone.reserve(t * 768);
        for i in 0..t {
            let src = (first_frame + i / 2).min(n_frames - 1);
            self.phone
                .extend_from_slice(&self.feats[src * 768..(src + 1) * 768]);
        }
        let pitch: Vec<i64> = self.f0[..t].iter().map(|&f| f0::coarse(f)).collect();
        let pitchf: Vec<f32> = self.f0[..t].to_vec();
        self.rnd.resize(192 * t, 0.0);
        self.rng.fill(&mut self.rnd);
        if (noise_scale - 1.0).abs() > 1e-3 {
            for v in self.rnd.iter_mut() {
                *v *= noise_scale;
            }
        }

        let outputs = self.voice.run(ort::inputs![
            "phone" => Tensor::from_array((vec![1i64, t as i64, 768], std::mem::take(&mut self.phone)))?,
            "phone_lengths" => Tensor::from_array((vec![1i64], vec![t as i64]))?,
            "pitch" => Tensor::from_array((vec![1i64, t as i64], pitch))?,
            "pitchf" => Tensor::from_array((vec![1i64, t as i64], pitchf))?,
            "ds" => Tensor::from_array((vec![1i64], vec![speaker]))?,
            "rnd" => Tensor::from_array((vec![1i64, 192, t as i64], std::mem::take(&mut self.rnd)))?,
        ])?;
        let (_shape, audio) = outputs["audio"].try_extract_tensor::<f32>()?;
        out.clear();
        out.extend_from_slice(audio);
        timings.synth_ms = t0.elapsed().as_secs_f32() * 1000.0;

        if self.model_rate.is_none() {
            // 100 frames per second of output: hop = samples per frame.
            let hop = out.len() as f64 / t as f64;
            let rate = (hop * 100.0).round() as usize;
            let snapped = [32_000usize, 40_000, 44_100, 48_000]
                .into_iter()
                .min_by_key(|r| r.abs_diff(rate))
                .ok_or_else(|| anyhow!("rate"))?;
            log::info!("voice model output rate detected as {snapped} Hz (hop {hop:.1})");
            self.model_rate = Some(snapped);
        }
        Ok(timings)
    }
}
