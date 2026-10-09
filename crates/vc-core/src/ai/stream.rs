//! Realtime glue: a lock-free stage for the audio thread and the worker
//! thread that runs inference on overlapping blocks with crossfades.

use super::resample::Resampler;
use super::rvc::{ModelPaths, Rvc};
use crate::VcParams;
use nice_plug::prelude::AtomicF32;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct StreamConfig {
    pub sample_rate: usize,
    pub crossfade_ms: f32,
    pub threads: usize,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            crossfade_ms: 60.0,
            threads: 8,
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiState {
    Off = 0,
    Loading = 1,
    Ready = 2,
    Error = 3,
}

impl AiState {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => AiState::Loading,
            2 => AiState::Ready,
            3 => AiState::Error,
            _ => AiState::Off,
        }
    }
}

/// Worker → UI.
#[derive(Default)]
pub struct AiStatus {
    state: AtomicU8,
    /// Whether the loaded voice has a retrieval index.
    pub has_index: AtomicBool,
    pub message: Mutex<String>,
    pub infer_ms: AtomicF32,
    pub block_ms: AtomicF32,
    pub f0_hz: AtomicF32,
    pub dropouts: AtomicU64,
    pub model_rate: AtomicU64,
}

impl AiStatus {
    pub fn state(&self) -> AiState {
        AiState::from_u8(self.state.load(Ordering::Relaxed))
    }
    fn set(&self, state: AiState, msg: impl Into<String>) {
        self.state.store(state as u8, Ordering::Relaxed);
        if let Ok(mut m) = self.message.lock() {
            *m = msg.into();
        }
    }
    pub fn message(&self) -> String {
        self.message.lock().map(|m| m.clone()).unwrap_or_default()
    }
    /// Inference time relative to the block length (>1 means it can't keep up).
    pub fn load_factor(&self) -> f32 {
        let b = self.block_ms.load(Ordering::Relaxed);
        if b <= 0.0 {
            0.0
        } else {
            self.infer_ms.load(Ordering::Relaxed) / b
        }
    }
}

/// Lives on the audio thread. Realtime-safe.
pub struct AiStage {
    input: rtrb::Producer<f32>,
    output: rtrb::Consumer<f32>,
    active: Arc<AtomicBool>,
    status: Arc<AiStatus>,
    latency: Arc<AtomicUsize>,
    seen_latency: usize,
    primed: bool,
    was_active: bool,
}

impl AiStage {
    pub fn status(&self) -> Arc<AiStatus> {
        self.status.clone()
    }

    /// Whether AI conversion is currently in the signal path.
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    pub fn latency_samples(&self) -> usize {
        self.latency.load(Ordering::Relaxed)
    }

    /// True once after activity or block size flipped (so hosts can re-report latency).
    pub fn take_active_changed(&mut self) -> bool {
        let now = self.is_active();
        let latency = self.latency_samples();
        let changed = now != self.was_active || (now && latency != self.seen_latency);
        if latency != self.seen_latency {
            self.seen_latency = latency;
            self.primed = false;
        }
        self.was_active = now;
        changed
    }

    /// Replace `buf` with converted audio. Returns false (buffer untouched)
    /// when AI is not active.
    pub fn process(&mut self, buf: &mut [f32]) -> bool {
        if !self.is_active() {
            self.primed = false;
            while self.output.pop().is_ok() {}
            return false;
        }
        for &x in buf.iter() {
            let _ = self.input.push(x);
        }
        if !self.primed {
            if self.output.slots() >= self.latency_samples() {
                self.primed = true;
            } else {
                buf.fill(0.0);
                return true;
            }
        }
        if self.output.slots() >= buf.len() {
            for x in buf.iter_mut() {
                *x = self.output.pop().unwrap_or(0.0);
            }
        } else {
            buf.fill(0.0);
            self.status.dropouts.fetch_add(1, Ordering::Relaxed);
        }
        true
    }
}

pub struct AiHandle {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for AiHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Finish the in-flight block so ONNX Runtime isn't torn down mid-run.
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub struct AiWorker;

impl AiWorker {
    /// Start the worker. It watches `params` (`ai_enabled`, `ai_voice`,
    /// `ai_pitch`) and loads models on demand.
    pub fn spawn(params: Arc<VcParams>, cfg: StreamConfig) -> (AiStage, AiHandle) {
        let sr = cfg.sample_rate;
        let (block_ms, _) = params.ai_speed.value().block_context_ms();
        let block = ms_to_samples(block_ms, sr);
        let crossfade = ms_to_samples(cfg.crossfade_ms, sr);
        let (in_prod, in_cons) = rtrb::RingBuffer::<f32>::new(sr * 4);
        let (out_prod, out_cons) = rtrb::RingBuffer::<f32>::new(sr * 4);
        let active = Arc::new(AtomicBool::new(false));
        let status = Arc::new(AiStatus::default());
        status.block_ms.store(block_ms, Ordering::Relaxed);
        let stop = Arc::new(AtomicBool::new(false));
        let latency = Arc::new(AtomicUsize::new(stage_latency(block, crossfade)));

        let stage = AiStage {
            input: in_prod,
            output: out_cons,
            active: active.clone(),
            status: status.clone(),
            latency: latency.clone(),
            seen_latency: 0,
            primed: false,
            was_active: false,
        };
        let thread = std::thread::Builder::new()
            .name("ai-worker".into())
            .spawn({
                let stop = stop.clone();
                move || {
                    worker_loop(
                        params, cfg, in_cons, out_prod, active, status, latency, stop,
                    )
                }
            })
            .expect("spawn ai worker");
        let handle = AiHandle {
            stop,
            thread: Some(thread),
        };
        (stage, handle)
    }
}

fn ms_to_samples(ms: f32, sr: usize) -> usize {
    (ms * 0.001 * sr as f32).round() as usize
}

/// Audio before the new block that the pitch model and synthesizer also see.
const SYNTH_CONTEXT_MS: f32 = if cfg!(feature = "gpu-rocm") {
    500.0
} else {
    200.0
};
/// Blocks after a (re)configuration during which timing is not judged:
/// GPU kernels compile for the new shapes and the first blocks are slow.
const SETTLE_BLOCKS: u32 = 4;
/// Consecutive slow blocks before Auto backs off.
const SLOW_BLOCKS_TO_BACK_OFF: u32 = 6;
/// How far the join may slide to line up waveform phase with the previous
/// block (SOLA), so successive blocks don't crossfade out of phase.
const SOLA_SEARCH_MS: f32 = 6.0;
/// Mean-square level below which a window counts as silence (about -80 dBFS).
const SILENCE_FLOOR: f32 = 1e-8;

/// One block waiting to be processed, half a block of inference headroom,
/// plus the crossfade region.
fn stage_latency(block: usize, crossfade: usize) -> usize {
    block + block / 2 + crossfade
}

struct Loaded {
    rvc: Rvc,
    to16k: Resampler,
    from_model: Option<Resampler>,
    voice: PathBuf,
    device: String,
    /// Speakers in the model; `ds` is clamped to this.
    speakers: i64,
}

#[allow(clippy::too_many_arguments)]
fn worker_loop(
    params: Arc<VcParams>,
    cfg: StreamConfig,
    mut input: rtrb::Consumer<f32>,
    mut output: rtrb::Producer<f32>,
    active: Arc<AtomicBool>,
    status: Arc<AiStatus>,
    latency: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
) {
    let sr = cfg.sample_rate;
    let crossfade = ms_to_samples(cfg.crossfade_ms, sr);
    let sola_search = ms_to_samples(SOLA_SEARCH_MS, sr);
    let tail_margin = ms_to_samples(20.0, sr) + sola_search;
    let mut speed = params.ai_speed.value();
    // What we actually run with; `Auto` may back this off at runtime.
    let mut effective = speed;
    let mut slow_blocks = 0u32;
    let mut settle = SETTLE_BLOCKS;
    let (mut block, mut context) = {
        let (b, c) = effective.block_context_ms();
        (ms_to_samples(b, sr), ms_to_samples(c, sr))
    };
    let mut window_len = context + block + crossfade + tail_margin;

    let mut loaded: Option<Loaded> = None;
    let mut history: Vec<f32> = vec![0.0; window_len];
    let mut audio16 = Vec::new();
    let mut synth = Vec::new();
    let mut out48 = Vec::new();
    let mut prev_tail: Vec<f32> = vec![0.0; crossfade];
    let mut block_buf = vec![0.0f32; block];
    let mut have_tail = false;
    let mut hpf = vc_dsp::biquad::Biquad::new(
        vc_dsp::biquad::FilterKind::HighPass,
        70.0,
        0.707,
        0.0,
        sr as f32,
    );

    while !stop.load(Ordering::Relaxed) {
        // ---- follow the parameters ---------------------------------------------
        let enabled = params.ai_enabled.value();
        let wanted = params
            .ai_voice
            .read()
            .map(|v| v.clone())
            .unwrap_or_default();
        let wanted_path = if wanted.is_empty() {
            None
        } else {
            Some(PathBuf::from(&wanted))
        };

        // Block-size changes: resize buffers and tell the stage to re-prime.
        let new_speed = params.ai_speed.value();
        let mut reconfigure = false;
        if new_speed != speed {
            speed = new_speed;
            effective = speed;
            slow_blocks = 0;
            reconfigure = true;
        }
        if speed == crate::AiSpeed::Auto && slow_blocks >= SLOW_BLOCKS_TO_BACK_OFF {
            if let Some(next) = effective.slower() {
                log::info!("AI can't keep up at {effective:?}; switching to {next:?}");
                effective = next;
                reconfigure = true;
            }
            slow_blocks = 0;
        }
        if reconfigure {
            let (b, c) = effective.block_context_ms();
            block = ms_to_samples(b, sr);
            context = ms_to_samples(c, sr);
            window_len = context + block + crossfade + tail_margin;
            history.clear();
            history.resize(window_len, 0.0);
            block_buf.resize(block, 0.0);
            have_tail = false;
            latency.store(stage_latency(block, crossfade), Ordering::Relaxed);
            status.block_ms.store(b, Ordering::Relaxed);
            while input.pop().is_ok() {}
            // New window shapes: compile/tune kernels now, not on live audio.
            if let Some(l) = loaded.as_mut() {
                let synth_context = ms_to_samples(SYNTH_CONTEXT_MS, 16_000);
                let tail16 = (block + crossfade + tail_margin) / 3 + synth_context;
                l.rvc.warm_up(l.to16k.output_len(window_len), tail16);
            }
            settle = SETTLE_BLOCKS;
        }

        let wanted_device = params.ai_device();
        let voice_changed = loaded.as_ref().map(|l| &l.voice) != wanted_path.as_ref()
            || loaded.as_ref().is_some_and(|l| l.device != wanted_device);
        if voice_changed {
            active.store(false, Ordering::Relaxed);
            release(loaded.take());
            if let Some(voice) = &wanted_path {
                status.set(
                    AiState::Loading,
                    format!(
                        "loading {}",
                        voice.file_name().unwrap_or_default().to_string_lossy()
                    ),
                );
                match load(voice, cfg.threads, &wanted_device) {
                    Ok(mut l) => {
                        let synth_context = ms_to_samples(SYNTH_CONTEXT_MS, 16_000);
                        let tail16 = (block + crossfade + tail_margin) / 3 + synth_context;
                        l.rvc.warm_up(l.to16k.output_len(window_len), tail16);
                        let backend =
                            super::compute::resolve(&wanted_device).label(&super::compute::gpus());
                        status
                            .has_index
                            .store(l.rvc.index.is_some(), Ordering::Relaxed);
                        loaded = Some(l);
                        settle = SETTLE_BLOCKS;
                        slow_blocks = 0;
                        status.set(AiState::Ready, backend);
                    }
                    Err(e) => {
                        log::error!("AI voice load failed: {e:#}");
                        status.set(AiState::Error, format!("{e:#}"));
                    }
                }
            } else {
                status.set(AiState::Off, "no voice selected");
            }
            history.fill(0.0);
            have_tail = false;
            while input.pop().is_ok() {}
        }

        let Some(l) = loaded.as_mut() else {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        };
        if !enabled {
            if active.swap(false, Ordering::Relaxed) {
                history.fill(0.0);
                have_tail = false;
            }
            while input.pop().is_ok() {}
            std::thread::sleep(Duration::from_millis(30));
            continue;
        }
        active.store(true, Ordering::Relaxed);

        // ---- wait for a block of input ------------------------------------------
        if input.slots() < block {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }
        for x in block_buf.iter_mut() {
            *x = input.pop().unwrap_or(0.0);
        }
        history.drain(..block);
        history.extend_from_slice(&block_buf);

        // ---- infer on the whole window ------------------------------------------
        // Digital silence makes the content encoder's normalization layers
        // divide by ~0 and emit NaN. Skip inference (emit silence) when the
        // window is effectively empty, and add a hair of dither otherwise.
        let energy = history.iter().map(|v| v * v).sum::<f32>() / history.len() as f32;
        if energy < SILENCE_FLOOR {
            for _ in 0..block {
                let _ = output.push(0.0);
            }
            have_tail = false;
            status.infer_ms.store(0.0, Ordering::Relaxed);
            continue;
        }
        let started = Instant::now();
        l.to16k.process(&history, &mut audio16);
        let mut dither = 0x2545_F491_4F6C_DD1Du64;
        for v in audio16.iter_mut() {
            dither ^= dither << 13;
            dither ^= dither >> 7;
            dither ^= dither << 17;
            *v += ((dither >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 2e-5;
        }
        let semitones = params.ai_pitch.value();
        // Synthesize the new block (+crossfade and margin) plus a little
        // context; the content encoder still sees the whole window.
        let synth_context = ms_to_samples(SYNTH_CONTEXT_MS, 16_000);
        let tail16 = (block + crossfade + tail_margin) / 3 + synth_context;
        // Breathiness 50% maps to the model's native noise level; below that
        // the excitation is tamed, above it exaggerated.
        let noise_scale = (params.ai_breath.value() * 2.0).clamp(0.0, 2.0);
        l.rvc.index_rate = if params.ai_index_enabled.value() {
            params.ai_index_rate.value()
        } else {
            0.0
        };
        let speaker = (params.ai_speaker.value() as i64).clamp(0, l.speakers - 1);
        let result = l.rvc.infer_tail_with(
            &audio16,
            tail16,
            semitones,
            speaker,
            noise_scale,
            &mut synth,
        );
        let timings = match result {
            Ok(t) => t,
            Err(e) => {
                log::error!("AI inference failed: {e:#}");
                status.set(AiState::Error, format!("{e:#}"));
                active.store(false, Ordering::Relaxed);
                release(loaded.take());
                continue;
            }
        };
        let model_rate = l.rvc.model_rate().unwrap_or(sr);
        status
            .model_rate
            .store(model_rate as u64, Ordering::Relaxed);
        if l.from_model.is_none() && model_rate != sr {
            l.from_model = Some(Resampler::new(model_rate, sr));
        }
        let out: &Vec<f32> = match &l.from_model {
            Some(r) => {
                r.process(&synth, &mut out48);
                &out48
            }
            None => &synth,
        };
        let infer_ms = started.elapsed().as_secs_f32() * 1000.0;
        status.infer_ms.store(infer_ms, Ordering::Relaxed);
        status.f0_hz.store(l.rvc.last_f0_hz, Ordering::Relaxed);
        let _ = timings;
        if settle > 0 {
            settle -= 1;
        } else if infer_ms > 0.85 * block as f32 / sr as f32 * 1000.0 {
            slow_blocks += 1;
        } else {
            slow_blocks = slow_blocks.saturating_sub(1);
        }

        // ---- keep the newest block (+crossfade), align, crossfade, push ----------
        // `seg` holds search + block + crossfade samples: the join may start
        // anywhere in the first `sola_search` of it.
        let need = block + crossfade + tail_margin;
        if out.len() < need {
            log::warn!("AI produced {} samples, need {need}", out.len());
            continue;
        }
        let seg_all = &out[out.len() - need..out.len() - tail_margin + sola_search];
        // SOLA: pick the offset whose first `crossfade` samples correlate best
        // with the previous block's tail.
        let offset = if have_tail {
            let mut best = (0usize, f32::MIN);
            for k in 0..=sola_search {
                let cand = &seg_all[k..k + crossfade];
                let (mut dot, mut e) = (0.0f32, 1e-9f32);
                for (a, b) in prev_tail.iter().zip(cand) {
                    dot += a * b;
                    e += b * b;
                }
                let score = dot / e.sqrt();
                if score > best.1 {
                    best = (k, score);
                }
            }
            best.0
        } else {
            0
        };
        let seg = &seg_all[offset..offset + block + crossfade];
        if seg.iter().any(|v| !v.is_finite()) {
            log::warn!("AI produced non-finite samples; emitting silence for this block");
            status.dropouts.fetch_add(1, Ordering::Relaxed);
            for _ in 0..block {
                let _ = output.push(0.0);
            }
            have_tail = false;
            continue;
        }
        for i in 0..block {
            let s = if i < crossfade && have_tail {
                let t = (i as f32 + 0.5) / crossfade as f32;
                let theta = t * std::f32::consts::FRAC_PI_2;
                let (a, b) = theta.sin_cos();
                prev_tail[i] * b + seg[i] * a
            } else {
                seg[i]
            };
            // 2nd-order high-pass at 70 Hz on the contiguous output stream
            // (vocoders emit sub-bass thumps on consonants).
            let _ = output.push(hpf.tick(s));
        }
        prev_tail.copy_from_slice(&seg[block..block + crossfade]);
        have_tail = true;
    }
    active.store(false, Ordering::Relaxed);
    release(loaded.take());
}

/// Drop a loaded model set. With the ROCm provider, tearing sessions down
/// corrupts the heap inside ONNX Runtime 1.22, so GPU builds leak them
/// instead (a voice switch costs its model size in GPU memory).
fn release(loaded: Option<Loaded>) {
    #[cfg(feature = "gpu-rocm")]
    std::mem::forget(loaded);
    #[cfg(not(feature = "gpu-rocm"))]
    drop(loaded);
}

fn load(voice: &std::path::Path, threads: usize, device: &str) -> anyhow::Result<Loaded> {
    let dir = super::models_dir().ok_or_else(|| anyhow::anyhow!("no data directory"))?;
    let (contentvec, rmvpe) = super::find_base_models(&dir).ok_or_else(|| {
        anyhow::anyhow!(
            "base models missing in {}: need contentvec_768l12_q8.onnx and rmvpe_q8.onnx",
            dir.display()
        )
    })?;
    let backend = super::compute::resolve(device);
    log::info!(
        "loading AI voice on {}",
        backend.label(&super::compute::gpus())
    );
    let mut rvc = Rvc::load_on(
        &ModelPaths {
            contentvec,
            rmvpe,
            voice: voice.to_path_buf(),
        },
        threads,
        backend,
    )?;
    let index_path = voice.with_extension("index");
    if index_path.is_file() {
        match super::index::RetrievalIndex::load(&index_path) {
            Ok(idx) => {
                log::info!(
                    "retrieval index: {} vectors, {:.0} MB in memory",
                    idx.ntotal,
                    idx.memory_bytes() as f32 / 1e6
                );
                rvc.index = Some(idx);
            }
            Err(e) => log::warn!("ignoring {}: {e:#}", index_path.display()),
        }
    }
    let speakers = super::library::speaker_count_for(voice).max(1) as i64;
    if speakers > 1 {
        log::info!("voice model has {speakers} speakers");
    }
    Ok(Loaded {
        rvc,
        to16k: Resampler::new(48_000, 16_000),
        from_model: None,
        speakers,
        voice: voice.to_path_buf(),
        device: device.to_string(),
    })
}
