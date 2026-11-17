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
    /// Inference threads. `0` follows the parameters (`ai_threads`, where
    /// 0 again means Auto: derived from the machine's cores).
    pub threads: usize,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            crossfade_ms: 60.0,
            threads: 0,
        }
    }
}

/// How long a voice that failed to load (or kept failing to run) is left
/// alone before another attempt, unless the user changes something.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// Consecutive inference failures before the voice is parked.
const MAX_INFER_FAILURES: u32 = 3;

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
    /// The model is loaded but not yet producing; the live voice is passing
    /// through in the meantime.
    pub warming: AtomicBool,
    pub message: Mutex<String>,
    pub infer_ms: AtomicF32,
    pub block_ms: AtomicF32,
    pub f0_hz: AtomicF32,
    pub dropouts: AtomicU64,
    pub model_rate: AtomicU64,
    /// Inference threads the loaded voice runs with (0 until loaded).
    pub threads: AtomicUsize,
    /// Coughs, burps and the like silenced since the voice was loaded.
    pub blocked: AtomicU64,
    /// The sound-event model is loaded and the blocker is working.
    pub blocker_ready: AtomicBool,
    /// The speaker's typical pitch in Hz as learned so far (0 = not yet).
    pub speaker_f0_hz: AtomicF32,
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
    /// Cushion kept between the worker and the audio thread, in samples:
    /// how late a block may arrive before the live voice has to fill in.
    margin: usize,
    /// Samples still to wait after the first block arrived before playing it.
    hold: usize,
    /// Lowest fill level seen since the last check, and samples since then.
    low_water: usize,
    since_check: usize,
    check_every: usize,
    was_active: bool,
    /// 0 = live voice, 1 = fully converted. Ramped so handing over to and
    /// from the model does not click.
    fade: f32,
}

/// How late a block may be before the live voice fills in. Inference time
/// varies by a few tens of milliseconds from block to block.
const MARGIN_MS: f32 = 50.0;

/// ~20 ms at 48 kHz for the AI/live-voice handover.
const AI_FADE_SAMPLES: f32 = 960.0;

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

    /// Mix converted audio into `buf`. Returns true only when the model
    /// actually replaced the audio. While it is warming up, or when it has
    /// dropped a block, the live voice already in `buf` is left alone -- the
    /// microphone must never go silent just because the AI is not ready.
    pub fn process(&mut self, buf: &mut [f32]) -> bool {
        if !self.is_active() {
            self.primed = false;
            self.fade = 0.0;
            self.status.warming.store(false, Ordering::Relaxed);
            while self.output.pop().is_ok() {}
            return false;
        }
        for &x in buf.iter() {
            let _ = self.input.push(x);
        }
        if !self.primed {
            // Blocks arrive one block apart, so playing the first one the
            // moment it lands would leave nothing in hand when the next is a
            // little late. Wait out the margin first; no more than that,
            // because every sample waited here is delay for good.
            if self.output.slots() == 0 {
                self.hold = self.margin;
            } else if self.hold > buf.len() {
                self.hold -= buf.len();
            } else {
                self.primed = true;
                self.low_water = usize::MAX;
                self.since_check = 0;
            }
            if !self.primed {
                // Still filling the model's first window.
                self.status.warming.store(true, Ordering::Relaxed);
                self.ease_out(buf.len());
                return false;
            }
        }
        if self.output.slots() < buf.len() {
            // The worker fell behind; keep this block of the live voice.
            self.status.dropouts.fetch_add(1, Ordering::Relaxed);
            self.status.warming.store(true, Ordering::Relaxed);
            self.ease_out(buf.len());
            return false;
        }
        self.status.warming.store(false, Ordering::Relaxed);
        let step = 1.0 / AI_FADE_SAMPLES;
        for x in buf.iter_mut() {
            let converted = self.output.pop().unwrap_or(0.0);
            self.fade = (self.fade + step).min(1.0);
            let (w_ai, w_live) = (self.fade * core::f32::consts::FRAC_PI_2).sin_cos();
            *x = converted * w_ai + *x * w_live;
        }
        self.reclaim_delay(buf.len());
        true
    }

    /// Every late block pushes the converted voice a little further behind
    /// (the audio thread waits instead of skipping). After one bad moment
    /// that delay would stay for the rest of the session, so when the queue
    /// has not come near empty for a while, the surplus is dropped.
    fn reclaim_delay(&mut self, frames: usize) {
        self.low_water = self.low_water.min(self.output.slots());
        self.since_check += frames;
        if self.since_check < self.check_every {
            return;
        }
        let surplus = self.low_water.saturating_sub(self.margin);
        if surplus > self.latency_samples() / 4
            && let Ok(chunk) = self.output.read_chunk(surplus)
        {
            chunk.commit_all();
            // Hand over through the live voice so the cut is not a click.
            self.fade = 0.0;
        }
        self.low_water = usize::MAX;
        self.since_check = 0;
    }

    /// Let the converted voice recede so the next handover fades back in.
    fn ease_out(&mut self, frames: usize) {
        self.fade = (self.fade - frames as f32 / AI_FADE_SAMPLES).max(0.0);
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
        // The worker settles the real sizes once it knows where the models
        // run; this is only the first guess for the reported latency.
        let block_ms = params
            .ai_speed
            .value()
            .block(cfg!(feature = "gpu"))
            .block_ms;
        let block = ms_to_samples(block_ms, sr);
        let crossfade = ms_to_samples(cfg.crossfade_ms, sr);
        let (in_prod, in_cons) = rtrb::RingBuffer::<f32>::new(sr * 4);
        let (out_prod, out_cons) = rtrb::RingBuffer::<f32>::new(sr * 4);
        let active = Arc::new(AtomicBool::new(false));
        let status = Arc::new(AiStatus::default());
        status.block_ms.store(block_ms, Ordering::Relaxed);
        let stop = Arc::new(AtomicBool::new(false));
        let latency = Arc::new(AtomicUsize::new(stage_latency(sr, block, crossfade)));

        let stage = AiStage {
            input: in_prod,
            output: out_cons,
            active: active.clone(),
            status: status.clone(),
            latency: latency.clone(),
            seen_latency: 0,
            primed: false,
            margin: ms_to_samples(MARGIN_MS, sr),
            hold: 0,
            low_water: usize::MAX,
            since_check: 0,
            check_every: sr * 10,
            was_active: false,
            fade: 0.0,
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

/// Remembers the pitch of the last stretches of speech to know what is
/// normal for this speaker.
struct PitchMemory {
    /// Median pitch of recent voiced blocks, newest last.
    recent: std::collections::VecDeque<f32>,
    typical: f32,
    since_update: usize,
}

impl PitchMemory {
    /// Voiced blocks kept (about a minute of talking).
    const KEEP: usize = 240;
    /// Blocks needed before the typical pitch is trusted.
    const ENOUGH: usize = 25;

    fn new(known: f32) -> Self {
        Self {
            recent: std::collections::VecDeque::with_capacity(Self::KEEP + 1),
            typical: if known > 40.0 { known } else { 0.0 },
            since_update: 0,
        }
    }

    /// Note one block's median pitch (0 = unvoiced, ignored). Returns true
    /// when the typical pitch was updated.
    fn add(&mut self, hz: f32) -> bool {
        if !(40.0..=800.0).contains(&hz) {
            return false;
        }
        self.recent.push_back(hz);
        if self.recent.len() > Self::KEEP {
            self.recent.pop_front();
        }
        self.since_update += 1;
        if self.recent.len() < Self::ENOUGH || self.since_update < Self::ENOUGH {
            return false;
        }
        self.since_update = 0;
        let mut sorted: Vec<f32> = self.recent.iter().copied().collect();
        sorted.sort_by(|a, b| a.total_cmp(b));
        self.typical = sorted[sorted.len() / 2];
        true
    }

    fn typical(&self) -> f32 {
        self.typical
    }

    /// The range this speaker can plausibly produce: an octave below the
    /// typical pitch to an octave and a half above (excited speech and
    /// laughter go high). The wide default until the pitch is known.
    fn range(&self) -> (f32, f32) {
        if self.typical > 0.0 {
            (self.typical * 0.5, self.typical * 2.8)
        } else {
            super::rvc::DEFAULT_F0_RANGE
        }
    }
}

fn ms_to_samples(ms: f32, sr: usize) -> usize {
    (ms * 0.001 * sr as f32).round() as usize
}

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

/// The delay through the stage for a sound that starts right at the
/// beginning of a block: the block has to fill, be converted (taken as
/// two-thirds of a block, about what a machine that keeps up needs), sit
/// out the margin, and the newest audio is held back for the crossfade and
/// the vocoder's unreliable edge.
fn stage_latency(sr: usize, block: usize, crossfade: usize) -> usize {
    let edge = ms_to_samples(20.0 + SOLA_SEARCH_MS, sr);
    block + block * 2 / 3 + ms_to_samples(MARGIN_MS, sr) + crossfade + edge
}

struct Loaded {
    rvc: Rvc,
    to16k: Resampler,
    from_model: Option<Resampler>,
    voice: PathBuf,
    device: String,
    threads: usize,
    /// Speakers in the model; `ds` is clamped to this.
    speakers: i64,
}

/// What the worker is asked to run: a change in any of these reloads.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Wanted {
    voice: PathBuf,
    device: String,
    threads: usize,
}

/// A configuration that failed, left alone until `until` or until the
/// user asks for something else. Without this the worker would retry a
/// missing or broken model twenty times a second (and, on GPU builds,
/// leak a session every time).
struct Parked {
    wanted: Wanted,
    until: Instant,
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
    // Whether the models run on a GPU; decides how much context is
    // affordable. Follows the Compute setting (and a GPU that failed).
    let mut gpu = cfg!(feature = "gpu");
    let (mut block, mut context, mut synth_context) = {
        let b = effective.block(gpu);
        (
            ms_to_samples(b.block_ms, sr),
            ms_to_samples(b.context_ms, sr),
            ms_to_samples(b.synth_context_ms, 16_000),
        )
    };
    let mut window_len = context + block + crossfade + tail_margin;
    // How long the last real conversion took: silent blocks are held back
    // by the same time so the output keeps its rhythm (see below).
    let mut typical_infer = Duration::ZERO;

    let mut loaded: Option<Loaded> = None;
    let mut parked: Option<Parked> = None;
    let mut infer_failures = 0u32;
    let mut last_input = Instant::now();
    let mut history: Vec<f32> = vec![0.0; window_len];
    let mut audio16 = Vec::new();
    let mut synth = Vec::new();
    let mut out48 = Vec::new();
    let mut prev_tail: Vec<f32> = vec![0.0; crossfade];
    let mut block_buf = vec![0.0f32; block];
    let mut have_tail = false;
    // Loudness follow, cough blocker, the speaker's own pitch.
    let mut gains: Vec<f32> = Vec::new();
    let mut detector: Option<super::events::EventDetector> = None;
    let mut detector_failed = false;
    let mut blocker = super::events::Blocker::default();
    let mut was_muting = false;
    let mut mute_gain = 1.0f32;
    let mute_step = 1.0 / ms_to_samples(10.0, sr) as f32;
    let mut pitch_memory = PitchMemory::new(params.my_pitch_hz());
    status
        .speaker_f0_hz
        .store(pitch_memory.typical(), Ordering::Relaxed);
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
        let now_gpu = super::compute::resolve(&params.ai_device()) != super::compute::Backend::Cpu;
        if now_gpu != gpu {
            gpu = now_gpu;
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
            let b = effective.block(gpu);
            block = ms_to_samples(b.block_ms, sr);
            context = ms_to_samples(b.context_ms, sr);
            synth_context = ms_to_samples(b.synth_context_ms, 16_000);
            window_len = context + block + crossfade + tail_margin;
            history.clear();
            history.resize(window_len, 0.0);
            block_buf.resize(block, 0.0);
            have_tail = false;
            latency.store(stage_latency(sr, block, crossfade), Ordering::Relaxed);
            status.block_ms.store(b.block_ms, Ordering::Relaxed);
            typical_infer = Duration::ZERO;
            while input.pop().is_ok() {}
            // New window shapes: compile/tune kernels now, not on live audio.
            if let Some(l) = loaded.as_mut() {
                let tail16 = (block + crossfade + tail_margin) / 3 + synth_context;
                l.rvc.warm_up(l.to16k.output_len(window_len), tail16);
            }
            settle = SETTLE_BLOCKS;
        }

        let wanted_device = params.ai_device();
        let wanted = wanted_path.as_ref().map(|voice| {
            let gpu = super::compute::resolve(&wanted_device) != super::compute::Backend::Cpu;
            let threads = if cfg.threads > 0 {
                cfg.threads
            } else {
                super::hardware::resolve_threads(params.ai_threads(), gpu)
            };
            Wanted {
                voice: voice.clone(),
                device: wanted_device.clone(),
                threads,
            }
        });
        let have = loaded.as_ref().map(|l| Wanted {
            voice: l.voice.clone(),
            device: l.device.clone(),
            threads: l.threads,
        });
        // A parked (failed) configuration is retried only after a pause, or
        // as soon as the user asks for something different.
        if let Some(p) = &parked
            && (wanted.as_ref() != Some(&p.wanted) || Instant::now() >= p.until)
        {
            parked = None;
        }
        let blocked = parked.is_some();
        if have != wanted && !blocked {
            active.store(false, Ordering::Relaxed);
            release(loaded.take());
            if let Some(w) = &wanted {
                status.set(
                    AiState::Loading,
                    format!(
                        "loading {}",
                        w.voice.file_name().unwrap_or_default().to_string_lossy()
                    ),
                );
                // DirectML can crash instead of failing: leave a marker
                // until the voice has loaded and run once (see `compute`).
                let directml = matches!(
                    super::compute::resolve(&w.device),
                    super::compute::Backend::DirectMl { .. }
                ) && super::compute::directml_begin();
                match load(&w.voice, w.threads, &w.device, sr) {
                    Ok(mut l) => {
                        let tail16 = (block + crossfade + tail_margin) / 3 + synth_context;
                        l.rvc.warm_up(l.to16k.output_len(window_len), tail16);
                        if directml {
                            super::compute::directml_ok();
                        }
                        let backend =
                            super::compute::resolve(&w.device).label(&super::compute::gpus());
                        status
                            .has_index
                            .store(l.rvc.index.is_some(), Ordering::Relaxed);
                        status.threads.store(w.threads, Ordering::Relaxed);
                        loaded = Some(l);
                        settle = SETTLE_BLOCKS;
                        slow_blocks = 0;
                        infer_failures = 0;
                        status.set(AiState::Ready, backend);
                    }
                    Err(e) => {
                        if directml {
                            super::compute::directml_ok();
                        }
                        log::error!("AI voice load failed: {e:#}");
                        status.set(AiState::Error, format!("{e:#}"));
                        parked = Some(Parked {
                            wanted: w.clone(),
                            until: Instant::now() + RETRY_AFTER,
                        });
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
            while input.pop().is_ok() {}
            std::thread::sleep(Duration::from_millis(if blocked { 200 } else { 50 }));
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
            // Poll briskly while audio is flowing; back off when nothing has
            // arrived for a while (no microphone, capture suspended).
            let idle = last_input.elapsed() > Duration::from_secs(1);
            std::thread::sleep(Duration::from_millis(if idle { 30 } else { 5 }));
            continue;
        }
        last_input = Instant::now();
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
            // A converted block reaches the audio thread one inference time
            // after its input is complete. Keep that rhythm for silent
            // blocks: delivered at once, they would let the queue run ahead
            // and the first spoken block afterwards would arrive "late".
            std::thread::sleep(typical_infer);
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
        // ---- is this a cough, a burp, a sneeze? -----------------------------------
        let block_noises = params.ai_block_noises.value();
        if block_noises && detector.is_none() && !detector_failed {
            match super::events::model_path() {
                Some(path) => match super::events::EventDetector::load(&path) {
                    Ok(d) => {
                        log::info!("cough and burp blocker ready");
                        detector = Some(d);
                    }
                    Err(e) => {
                        log::warn!("the cough and burp blocker could not start: {e:#}");
                        detector_failed = true;
                    }
                },
                // Not downloaded: look again now and then, it may arrive.
                None => detector_failed = false,
            }
        }
        status
            .blocker_ready
            .store(block_noises && detector.is_some(), Ordering::Relaxed);
        let mute = match detector.as_mut().filter(|_| block_noises) {
            Some(d) => match d.listen(&audio16) {
                Ok(heard) => {
                    let mute = blocker.decide(&heard);
                    if mute && !was_muting {
                        status.blocked.fetch_add(1, Ordering::Relaxed);
                        log::debug!("blocked a {} ({:.2})", heard.class_name(), heard.blocked);
                    }
                    mute
                }
                Err(e) => {
                    log::warn!("sound classifier failed, blocker off: {e:#}");
                    detector = None;
                    detector_failed = true;
                    false
                }
            },
            None => {
                blocker.reset();
                false
            }
        };
        was_muting = mute;

        let semitones = params.ai_pitch.value();
        // Synthesize the new block (+crossfade and margin) plus a little
        // context; the content encoder still sees the whole window.
        let tail16 = (block + crossfade + tail_margin) / 3 + synth_context;
        // Breathiness 50% maps to the model's native noise level; below that
        // the excitation is tamed, above it exaggerated.
        let noise_scale = (params.ai_breath.value() * 2.0).clamp(0.0, 2.0);
        l.rvc.index_rate = if params.ai_index_enabled.value() {
            params.ai_index_rate.value()
        } else {
            0.0
        };
        // 0% keeps the index everywhere; 100% leaves unvoiced frames alone.
        l.rvc.protect = 1.0 - params.ai_protect.value().clamp(0.0, 1.0);
        l.rvc.f0_range = pitch_memory.range();
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
            Ok(t) => {
                infer_failures = 0;
                t
            }
            Err(e) => {
                infer_failures += 1;
                log::error!("AI inference failed ({infer_failures}): {e:#}");
                status.set(AiState::Error, format!("{e:#}"));
                active.store(false, Ordering::Relaxed);
                let taken = loaded.take();
                let was = taken.as_ref().map(|l| Wanted {
                    voice: l.voice.clone(),
                    device: l.device.clone(),
                    threads: l.threads,
                });
                release(taken);
                if infer_failures >= MAX_INFER_FAILURES
                    && let Some(w) = was
                {
                    // Reloading a model that fails every time just burns the
                    // CPU (and GPU memory); leave it until something changes.
                    log::error!(
                        "AI voice keeps failing; giving it a rest. Pick it again or change Compute to retry."
                    );
                    parked = Some(Parked {
                        wanted: w,
                        until: Instant::now() + RETRY_AFTER * 10,
                    });
                }
                // Let the model's session go through the normal release path
                // and give the system a moment before reloading.
                std::thread::sleep(Duration::from_millis(250));
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
        let out: &mut Vec<f32> = match &l.from_model {
            Some(r) => {
                r.process(&synth, &mut out48);
                &mut out48
            }
            None => &mut synth,
        };
        // Learn the speaker's own pitch from what was just said.
        if pitch_memory.add(l.rvc.last_input_f0_hz) {
            let typical = pitch_memory.typical();
            status.speaker_f0_hz.store(typical, Ordering::Relaxed);
            params.set_my_pitch_hz(typical);
        }
        typical_infer = started.elapsed();
        let infer_ms = typical_infer.as_secs_f32() * 1000.0;
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
        // The newest `need` samples of the output and of the input window
        // are the same stretch of time: make the one as loud as the other.
        let follow = params.ai_volume_follow.value();
        if follow > 0.0 {
            let produced = out.len();
            super::envelope::follow(
                &history[window_len - need..],
                &mut out[produced - need..],
                follow,
                &mut gains,
            );
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
            // A blocked sound fades to silence over 10 ms and back.
            let target = if mute { 0.0 } else { 1.0 };
            mute_gain += (target - mute_gain).clamp(-mute_step, mute_step);
            // 2nd-order high-pass at 70 Hz on the contiguous output stream
            // (vocoders emit sub-bass thumps on consonants).
            let _ = output.push(hpf.tick(s) * mute_gain);
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
    #[cfg(feature = "gpu")]
    std::mem::forget(loaded);
    #[cfg(not(feature = "gpu"))]
    drop(loaded);
}

fn load(
    voice: &std::path::Path,
    threads: usize,
    device: &str,
    sample_rate: usize,
) -> anyhow::Result<Loaded> {
    if !voice.is_file() {
        anyhow::bail!(
            "the voice file is missing: {} (pick another voice on the Voices page)",
            voice.display()
        );
    }
    let dir = super::models_dir().ok_or_else(|| anyhow::anyhow!("no data directory"))?;
    let (contentvec, rmvpe) = super::find_base_models(&dir).ok_or_else(|| {
        anyhow::anyhow!(
            "the base models are not downloaded yet (Voices page → Download base models); looked for {} and {} in {}",
            super::CONTENTVEC_FILES[0],
            super::RMVPE_FILES[0],
            dir.display()
        )
    })?;
    let backend = super::compute::resolve(device);
    log::info!(
        "loading AI voice on {} with {threads} thread{}",
        backend.label(&super::compute::gpus()),
        if threads == 1 { "" } else { "s" }
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
        to16k: Resampler::new(sample_rate, 16_000),
        from_model: None,
        speakers,
        voice: voice.to_path_buf(),
        device: device.to_string(),
        threads,
    })
}
