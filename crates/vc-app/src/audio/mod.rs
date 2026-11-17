//! Audio I/O. The backend (PipeWire on Linux, cpal elsewhere) owns the
//! devices and the realtime callbacks; everything it shares with the rest of
//! the app lives here, so the window and the remote control never see which
//! backend is running.

pub mod block;
pub mod fold;
#[cfg(any(not(target_os = "linux"), feature = "backend-cpal", test))]
pub mod resample;
pub mod sink;
pub mod slip;

#[cfg(target_os = "linux")]
#[cfg_attr(feature = "backend-cpal", allow(dead_code))]
pub mod pw;
#[cfg(all(target_os = "linux", not(feature = "backend-cpal")))]
pub use pw::{AudioThread, library_version};

#[cfg(any(not(target_os = "linux"), feature = "backend-cpal"))]
pub mod cpal;
#[cfg(any(not(target_os = "linux"), feature = "backend-cpal"))]
pub use self::cpal::{AudioThread, library_version};

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Name of the virtual microphone node (PipeWire only; kept here because the
/// command line names it on every platform).
pub const SOURCE_NODE_NAME: &str = "voice_changer.source";

/// Largest block a callback will ever be asked for.
pub(crate) const MAX_BLOCK: usize = 8192;

#[derive(Debug, Clone)]
pub struct AudioConfig {
    pub rate: u32,
    pub quantum: u32,
    pub capture_target: Option<String>,
    /// PipeWire `node.name` for the published microphone.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub source_name: String,
    /// Test input: loop this mono 48 kHz buffer instead of the microphone.
    pub input_wav: Option<Arc<Vec<f32>>>,
    /// Play the voice to this output device instead of the default one
    /// (PipeWire: the "Hear myself" sink, by `node.name`).
    pub output_target: Option<String>,
}

/// Where the audio side stands, for the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioState {
    Starting,
    Running,
    /// The daemon went away; a new session is being attempted.
    Reconnecting,
    /// Could not start at all (no PipeWire, refused connection).
    Failed,
}

#[derive(Debug, Clone)]
pub struct AudioHealth {
    pub state: AudioState,
    /// One plain sentence about the current problem, empty when fine.
    pub message: String,
    /// The virtual microphone node exists and is fed.
    pub virtual_mic: bool,
    /// The capture stream has delivered audio recently.
    pub mic_alive: bool,
    /// How many sessions have been (re)started.
    pub sessions: u32,
}

impl Default for AudioHealth {
    fn default() -> Self {
        Self {
            state: AudioState::Starting,
            message: String::new(),
            virtual_mic: false,
            // Optimistic until the one-second check has seen two quiet
            // seconds; otherwise every start would flash "no sound".
            mic_alive: true,
            sessions: 0,
        }
    }
}

/// An application that is currently recording (a `Stream/Input/Audio` node).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppStream {
    pub id: u32,
    pub serial: Option<String>,
    /// `application.name`, falling back to `node.name`.
    pub app: String,
    /// `media.name` (what the app calls the stream), may be empty.
    pub media: String,
    /// Whether the stream's `target.object` currently points at us.
    pub routed: bool,
}

/// A selectable microphone or output device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// False for a microphone, true for an output.
    pub output: bool,
    pub id: u32,
    pub serial: Option<String>,
    pub name: String,
    pub description: String,
}

/// The test bench: record a few seconds of the raw microphone, or replay a
/// clip in place of the microphone. A replayed clip goes through the whole
/// chain to "Hear myself" only; the virtual microphone carries silence
/// meanwhile, so nobody in a call ever hears a recording.
pub struct Bench {
    /// The clip replayed instead of the microphone, looped.
    pub(crate) play: arc_swap::ArcSwapOption<Vec<f32>>,
    /// Samples still to record; the capture callback counts it down.
    record_left: AtomicUsize,
    record_total: AtomicUsize,
    /// Pre-sized, so the realtime side never allocates. Only the capture
    /// callback touches it while `record_left` is above zero.
    recording: Mutex<Vec<f32>>,
}

impl Bench {
    /// Longest recording, in seconds.
    pub const MAX_SECONDS: usize = 20;

    pub(crate) fn new(rate: u32) -> Self {
        Self {
            play: arc_swap::ArcSwapOption::empty(),
            record_left: AtomicUsize::new(0),
            record_total: AtomicUsize::new(0),
            recording: Mutex::new(Vec::with_capacity(rate as usize * Self::MAX_SECONDS)),
        }
    }

    /// Start recording `samples` of the raw microphone.
    pub fn record(&self, samples: usize) {
        self.record_left.store(0, Ordering::SeqCst);
        if let Ok(mut buf) = self.recording.lock() {
            buf.clear();
            let samples = samples.min(buf.capacity());
            self.record_total.store(samples, Ordering::SeqCst);
            self.record_left.store(samples, Ordering::SeqCst);
        }
    }

    /// 0..1 while recording, `None` when idle.
    pub fn record_progress(&self) -> Option<f32> {
        let total = self.record_total.load(Ordering::Relaxed);
        let left = self.record_left.load(Ordering::Relaxed);
        (total > 0).then(|| 1.0 - left as f32 / total as f32)
    }

    /// The finished recording, once; `None` while recording or when idle.
    pub fn take_recording(&self) -> Option<Vec<f32>> {
        if self.record_total.load(Ordering::SeqCst) == 0
            || self.record_left.load(Ordering::SeqCst) > 0
        {
            return None;
        }
        let mut buf = self.recording.lock().ok()?;
        self.record_total.store(0, Ordering::SeqCst);
        let capacity = buf.capacity();
        Some(std::mem::replace(&mut *buf, Vec::with_capacity(capacity)))
    }

    /// Replay `clip` in place of the microphone (`None` = back to live).
    pub fn play(&self, clip: Option<Arc<Vec<f32>>>) {
        self.play.store(clip.filter(|c| !c.is_empty()));
    }

    pub fn playing(&self) -> bool {
        self.play.load().is_some()
    }

    /// Realtime: keep what the microphone delivered, if a recording runs.
    pub(crate) fn capture(&self, block: &[f32]) {
        let left = self.record_left.load(Ordering::Relaxed);
        if left == 0 {
            return;
        }
        // Uncontended: the other side only locks once `record_left` is zero.
        if let Ok(mut buf) = self.recording.try_lock() {
            let take = left.min(block.len()).min(buf.capacity() - buf.len());
            buf.extend_from_slice(&block[..take]);
            let rest = if take < block.len() { 0 } else { left - take };
            self.record_left.store(rest, Ordering::Relaxed);
        }
    }
}

/// Counters the realtime callbacks bump; read from anywhere.
#[derive(Default)]
pub struct Stats {
    pub capture_callbacks: AtomicU64,
    pub output_callbacks: AtomicU64,
    /// Output callbacks that found too little captured audio and emitted silence.
    pub underruns: AtomicU64,
    /// Captured frames dropped to keep latency bounded. Grows while nobody is
    /// listening to the virtual mic (the source node is suspended), so it is
    /// informational rather than an error.
    pub trimmed_frames: AtomicU64,
    /// Longest and (exponentially averaged) typical time spent processing one
    /// capture callback, in microseconds. Compare with the quantum period.
    pub process_max_us: AtomicU64,
    pub process_avg_us: AtomicU64,
    /// Capture callbacks skipped because the pipeline was busy elsewhere
    /// (only while a session is being torn down).
    pub skipped_callbacks: AtomicU64,
}

/// Flush denormal numbers to zero on this thread. Filters and reverb tails
/// decaying into silence otherwise produce denormals, which are up to a
/// hundred times slower to compute and show up as dropouts after you stop
/// talking.
#[inline]
pub(crate) fn no_denormals() {
    #[cfg(target_arch = "x86_64")]
    #[allow(deprecated)]
    // SAFETY: SSE is part of the x86_64 baseline; setting FTZ (bit 15) and
    // DAZ (bit 6) only changes how this thread rounds denormal floats.
    unsafe {
        use std::arch::x86_64::{_mm_getcsr, _mm_setcsr};
        _mm_setcsr(_mm_getcsr() | 0x8040);
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: sets the FZ bit (24) of this thread's floating-point control
    // register; no memory is touched.
    unsafe {
        let mut fpcr: u64;
        std::arch::asm!("mrs {}, fpcr", out(reg) fpcr);
        fpcr |= 1 << 24;
        std::arch::asm!("msr fpcr, {}", in(reg) fpcr);
    }
}

/// Queue a block for one of the sink callbacks. A full ring (nobody has been
/// reading for a second) drops the block; the reader trims anyway.
pub(crate) fn push_all(ring: &mut rtrb::Producer<f32>, block: &[f32], stats: Option<&Stats>) {
    match ring.write_chunk_uninit(block.len()) {
        Ok(chunk) => {
            chunk.fill_from_iter(block.iter().copied());
        }
        Err(_) => {
            if let Some(stats) = stats {
                stats
                    .trimmed_frames
                    .fetch_add(block.len() as u64, Ordering::Relaxed);
            }
        }
    }
}
