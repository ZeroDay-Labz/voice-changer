//! PipeWire I/O: one capture stream from the microphone, one output stream
//! feeding the virtual source ("Voice Changer Mic"), and an on-demand
//! monitor stream ("Hear myself").
//!
//! The streams live on a dedicated thread that runs the PipeWire main loop.
//! Their realtime `process` callbacks run on PipeWire's data thread and talk
//! to each other through lock-free SPSC ring buffers. The engine runs inside
//! the *capture* callback so processing never stops while nobody listens.
//!
//! The capture, the output and the virtual source share one `node.group`, so
//! PipeWire schedules them in the same graph cycle under one driver (the
//! microphone's clock) and nothing drifts between them. The monitor is left
//! out on purpose: it plays on the output device, which keeps its own clock
//! (grouping it would put all desktop playback on the microphone's clock);
//! the drift between the two is absorbed in the monitor callback. All four
//! share a `node.link-group`, which tells the session manager never to link
//! our capture to our own virtual microphone (a feedback loop) even while
//! that is the default source.
//!
//! The thread runs *sessions*: one connection to the daemon with all its
//! streams and listeners. When the daemon goes away (restart, crash, a
//! WirePlumber update) the session ends, the thread waits a moment and
//! starts a new one, re-applying the microphone choice and the take-over.
//! The engine and the loaded AI models survive the gap.

use anyhow::{Context as _, Result, anyhow};
use pipewire as pw;
use pw::proxy::ProxyT;
use pw::registry::GlobalObject;
use pw::spa;
use pw::spa::pod::Pod;
use pw::types::ObjectType;
use pw::{properties::properties, stream::StreamFlags};
use spa::param::format::{MediaSubtype, MediaType};
use spa::param::format_utils;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::ffi::CStr;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use vc_core::{Meters, Pipeline, VcParams};

use crate::fold::StereoFold;
use crate::slip::Slip;

pub const SOURCE_NODE_NAME: &str = "voice_changer.source";
pub const SOURCE_DESCRIPTION: &str = "Voice Changer Mic";
/// Internal stream that feeds the virtual microphone node.
const OUTPUT_NODE_NAME: &str = "voice_changer.output";
pub const CAPTURE_NODE_NAME: &str = "voice_changer.capture";
pub const MONITOR_NODE_NAME: &str = "voice_changer.monitor";

const SAMPLE_BYTES: usize = std::mem::size_of::<f32>();
/// Largest block we'll ever be asked for in one callback.
const MAX_BLOCK: usize = 8192;
/// Wait between reconnection attempts, doubling up to the maximum.
const RECONNECT_MIN: Duration = Duration::from_millis(500);
const RECONNECT_MAX: Duration = Duration::from_secs(5);

static PW_INIT: Once = Once::new();

fn ensure_init() {
    PW_INIT.call_once(pw::init);
}

pub fn library_version() -> String {
    ensure_init();
    // SAFETY: pw_get_library_version returns a static NUL-terminated string.
    unsafe { CStr::from_ptr(pw::sys::pw_get_library_version()) }
        .to_string_lossy()
        .into_owned()
}

#[derive(Debug, Clone)]
pub struct AudioConfig {
    pub rate: u32,
    pub quantum: u32,
    pub capture_target: Option<String>,
    /// PipeWire `node.name` for the published microphone.
    pub source_name: String,
    /// Test input: loop this mono 48 kHz buffer instead of the microphone.
    pub input_wav: Option<Arc<Vec<f32>>>,
    /// Play "Hear myself" to this sink (`node.name`) instead of the default
    /// output. Used by tests.
    pub monitor_target: Option<String>,
}

enum Command {
    Quit,
    /// Move the capture stream to another microphone (`node.name`), or back
    /// to the system default with `None`.
    SetCaptureTarget(Option<String>),
    /// Start/stop the headphone monitor stream.
    SetMonitor(bool),
    /// Point an application's capture stream at our virtual mic (or release it).
    RouteApp {
        stream_id: u32,
        on: bool,
    },
    /// Make the virtual mic the session's default source (or restore the previous one).
    SetDefaultSource(bool),
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

/// Session-manager state we read back from the `default` metadata object.
#[derive(Debug, Default)]
pub struct Routing {
    /// `object.serial` of our published source node, once the registry reports it.
    pub source_serial: Option<String>,
    /// Current `default.configured.audio.source` value (JSON), as reported.
    pub default_current: Option<String>,
    /// The value to restore when we stop being the default.
    pub default_prev: Option<String>,
    pub default_is_us: bool,
    /// `target.object` per stream node id.
    pub targets: std::collections::HashMap<u32, String>,
    /// Whether we are taking over as the microphone for every application.
    pub take_over: bool,
    /// Each app's `target.object` before we routed it to us (None = it was
    /// following the default), so we can put it back when we stop.
    pub original_targets: std::collections::HashMap<u32, Option<String>>,
}

impl Routing {
    /// Forget everything tied to a daemon instance (ids, serials, metadata
    /// values); keep what the user asked for.
    fn reset_for_new_session(&mut self) {
        self.source_serial = None;
        self.default_current = None;
        self.default_is_us = false;
        self.targets.clear();
        self.original_targets.clear();
    }
}

/// A selectable microphone (any `Audio/Source` node that isn't ours).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
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
    play: arc_swap::ArcSwapOption<Vec<f32>>,
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

    fn new(rate: u32) -> Self {
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
    fn capture(&self, block: &[f32]) {
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

pub struct AudioThread {
    sender: pw::channel::Sender<Command>,
    thread: Option<JoinHandle<Result<()>>>,
    pub stats: Arc<Stats>,
    #[cfg(feature = "ai")]
    pub ai_status: Arc<vc_core::ai::AiStatus>,
    devices: Arc<Mutex<Vec<DeviceInfo>>>,
    /// The microphone the user asked for (`None` = system default).
    capture_target: Arc<Mutex<Option<String>>>,
    /// The microphone actually in use right now: differs from the preferred
    /// one while that is unplugged.
    active_target: Arc<Mutex<Option<String>>>,
    monitor_on: Arc<AtomicBool>,
    app_streams: Arc<Mutex<Vec<AppStream>>>,
    routing: Arc<Mutex<Routing>>,
    health: Arc<Mutex<AudioHealth>>,
    /// Links from the virtual mic to applications (apps hearing the voice).
    listeners: Arc<AtomicUsize>,
    quit: Arc<AtomicBool>,
    source_name: String,
    pub bench: Arc<Bench>,
}

impl AudioThread {
    pub fn spawn(
        config: AudioConfig,
        params: Arc<VcParams>,
        stats: Arc<Stats>,
        meters: Arc<Meters>,
    ) -> Result<Self> {
        ensure_init();
        let (sender, receiver) = pw::channel::channel::<Command>();
        let devices = Arc::new(Mutex::new(Vec::new()));
        let capture_target = Arc::new(Mutex::new(config.capture_target.clone()));
        let active_target = Arc::new(Mutex::new(config.capture_target.clone()));
        let monitor_on = Arc::new(AtomicBool::new(false));
        let app_streams = Arc::new(Mutex::new(Vec::new()));
        let routing = Arc::new(Mutex::new(Routing::default()));
        let health = Arc::new(Mutex::new(AudioHealth::default()));
        let listeners = Arc::new(AtomicUsize::new(0));
        let quit = Arc::new(AtomicBool::new(false));
        let source_name = config.source_name.clone();
        let bench = Arc::new(Bench::new(config.rate));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();

        let pipeline = Pipeline::new(config.rate as f32, MAX_BLOCK, params);
        #[cfg(feature = "ai")]
        let ai_status = pipeline.ai_status();
        let ctx = LoopContext {
            config,
            pipeline: Arc::new(Mutex::new(pipeline)),
            stats: stats.clone(),
            meters: meters.clone(),
            devices: devices.clone(),
            capture_target: capture_target.clone(),
            active_target: active_target.clone(),
            monitor_on: monitor_on.clone(),
            app_streams: app_streams.clone(),
            routing: routing.clone(),
            health: health.clone(),
            listeners: listeners.clone(),
            quit: quit.clone(),
            bench: bench.clone(),
        };
        let thread = std::thread::Builder::new()
            .name("pipewire-main".into())
            .spawn(move || run_thread(ctx, receiver, ready_tx))?;

        // Surface startup errors (no daemon, bad target...) to the caller.
        ready_rx
            .recv()
            .map_err(|_| anyhow!("PipeWire thread died during startup"))??;

        Ok(Self {
            sender,
            thread: Some(thread),
            stats,
            #[cfg(feature = "ai")]
            ai_status,
            devices,
            capture_target,
            active_target,
            monitor_on,
            app_streams,
            routing,
            health,
            listeners,
            quit,
            source_name,
            bench,
        })
    }

    /// Applications recording right now, with whether they receive our voice.
    pub fn app_streams(&self) -> Vec<AppStream> {
        let routing = self.routing.lock().ok();
        let mut list = self
            .app_streams
            .lock()
            .map(|l| l.clone())
            .unwrap_or_default();
        for s in &mut list {
            s.routed = routing
                .as_ref()
                .and_then(|r| {
                    r.targets
                        .get(&s.id)
                        .map(|t| Some(t) == r.source_serial.as_ref() || *t == self.source_name)
                })
                .unwrap_or(false);
        }
        list.sort_by_key(|a| a.app.to_lowercase());
        list
    }

    pub fn route_app(&self, stream_id: u32, on: bool) {
        let _ = self.sender.send(Command::RouteApp { stream_id, on });
    }

    pub fn is_default_source(&self) -> bool {
        self.routing
            .lock()
            .map(|r| r.default_is_us)
            .unwrap_or(false)
    }

    /// Whether take-over is wanted (it may still be waiting for the session manager).
    pub fn take_over(&self) -> bool {
        self.routing.lock().map(|r| r.take_over).unwrap_or(false)
    }

    pub fn set_default_source(&self, on: bool) {
        let _ = self.sender.send(Command::SetDefaultSource(on));
    }

    /// Whether processed audio is also played to the default output.
    pub fn monitor(&self) -> bool {
        self.monitor_on.load(Ordering::Relaxed)
    }

    pub fn set_monitor(&self, on: bool) {
        self.monitor_on.store(on, Ordering::Relaxed);
        let _ = self.sender.send(Command::SetMonitor(on));
    }

    /// Microphones currently present in the graph.
    pub fn devices(&self) -> Vec<DeviceInfo> {
        self.devices.lock().map(|d| d.clone()).unwrap_or_default()
    }

    /// `node.name` of the microphone the user chose; `None` = system default.
    pub fn capture_target(&self) -> Option<String> {
        self.capture_target.lock().ok().and_then(|t| t.clone())
    }

    /// The microphone in use right now (differs from the choice while that
    /// one is unplugged).
    pub fn active_target(&self) -> Option<String> {
        self.active_target.lock().ok().and_then(|t| t.clone())
    }

    pub fn set_capture_target(&self, target: Option<String>) {
        if let Ok(mut t) = self.capture_target.lock() {
            *t = target.clone();
        }
        let _ = self.sender.send(Command::SetCaptureTarget(target));
    }

    pub fn health(&self) -> AudioHealth {
        self.health.lock().map(|h| h.clone()).unwrap_or_default()
    }

    /// Applications currently linked to the virtual microphone.
    pub fn listeners(&self) -> usize {
        self.listeners.load(Ordering::Relaxed)
    }

    pub fn shutdown(mut self) -> Result<()> {
        self.quit.store(true, Ordering::SeqCst);
        let _ = self.sender.send(Command::Quit);
        if let Some(t) = self.thread.take() {
            t.join()
                .map_err(|_| anyhow!("PipeWire thread panicked"))??;
        }
        Ok(())
    }
}

/// Capture callback: converts to mono, runs the whole pipeline, and feeds
/// the virtual-mic ring (and the monitor ring when enabled). Processing
/// lives here so it keeps running even when nobody is listening to the mic.
struct CaptureData {
    to_source: rtrb::Producer<f32>,
    to_monitor: rtrb::Producer<f32>,
    monitor_on: Arc<AtomicBool>,
    format: spa::param::audio::AudioInfoRaw,
    /// Picks the live channel(s) of a stereo input (see `fold.rs`).
    fold: StereoFold,
    left: Vec<f32>,
    right: Vec<f32>,
    /// Shared with the thread so it survives a reconnect. Only this callback
    /// ever locks it while a session runs; `try_lock` keeps the RT path free
    /// of blocking.
    pipeline: Arc<Mutex<Pipeline>>,
    scratch: Vec<f32>,
    input_wav: Option<Arc<Vec<f32>>>,
    wav_pos: usize,
    bench: Arc<Bench>,
    bench_pos: usize,
    stats: Arc<Stats>,
    meters: Arc<Meters>,
}

/// A playback-side callback that drains a ring: used for both the virtual
/// mic and the monitor.
struct SinkData {
    consumer: rtrb::Consumer<f32>,
    /// Interleaved channels of the stream; the mono voice goes to all of them.
    channels: usize,
    /// Keep roughly this many frames queued so a late producer doesn't
    /// starve us; trim anything beyond 2x it. Grows by one quantum on every
    /// underrun (up to `max_slack`), trading a little latency for
    /// silence-free output, and shrinks back after a calm stretch.
    target_slack: usize,
    min_slack: usize,
    quantum: usize,
    max_slack: usize,
    /// Frames delivered since the last underrun; `relax_after` of them
    /// without one take a quantum of slack back.
    calm_frames: usize,
    relax_after: usize,
    /// Output silence until the queue holds a block plus the slack, so the
    /// capture/output callback order within a cycle can't starve us.
    primed: bool,
    /// Frames of fade-in still to apply after a gap or a trim, so the voice
    /// comes back without a click.
    fade_left: usize,
    /// Set from the main thread to throw away whatever is queued (the monitor
    /// was just switched on: what the ring holds is stale).
    flush: Arc<AtomicBool>,
    stats: Arc<Stats>,
    count_callbacks: bool,
    /// Set for a stream that plays on another device's clock (the monitor):
    /// the fill level is steered by stretching blocks by one sample.
    drift: Option<Slip>,
    /// Samples popped from the ring for this block (one more than a block
    /// when catching up), and the block as it will be played.
    staging: Vec<f32>,
    block: Vec<f32>,
}

/// Length of the fade-in after a discontinuity (about 2.7 ms at 48 kHz).
const FADE_FRAMES: usize = 128;

impl SinkData {
    fn new(
        consumer: rtrb::Consumer<f32>,
        channels: usize,
        config: &AudioConfig,
        stats: Arc<Stats>,
        count_callbacks: bool,
        own_clock: bool,
    ) -> Self {
        let quantum = (config.quantum as usize).max(64);
        // In our own scheduling group the producer and this reader run once
        // each per cycle, so nothing needs to be held back (an unlucky
        // callback order shows up as one underrun and adds a quantum). On
        // another clock a quantum of cushion absorbs the two devices' jitter.
        let slack = if own_clock { quantum } else { 0 };
        Self {
            consumer,
            channels: channels.max(1),
            target_slack: slack,
            min_slack: slack,
            quantum,
            max_slack: quantum * 4,
            calm_frames: 0,
            relax_after: config.rate as usize * 30,
            primed: false,
            fade_left: 0,
            flush: Arc::new(AtomicBool::new(false)),
            stats,
            count_callbacks,
            drift: own_clock.then(Slip::default),
            staging: vec![0.0; MAX_BLOCK + 1],
            block: vec![0.0; MAX_BLOCK],
        }
    }

    /// Drop `frames` queued frames.
    fn discard(&mut self, frames: usize) {
        if let Ok(chunk) = self.consumer.read_chunk(frames.min(self.consumer.slots())) {
            chunk.commit_all();
        }
    }
}

fn drain_into(stream: &pw::stream::Stream, data: &mut SinkData) {
    if data.count_callbacks {
        data.stats.output_callbacks.fetch_add(1, Ordering::Relaxed);
    }
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };
    let requested = buffer.requested() as usize;
    let datas = buffer.datas_mut();
    let Some(d) = datas.first_mut() else { return };
    let Some(bytes) = d.data() else { return };
    let frame_bytes = SAMPLE_BYTES * data.channels;
    let max_frames = bytes.len() / frame_bytes;
    let n = if requested > 0 {
        requested.min(max_frames)
    } else {
        max_frames
    };

    if data.flush.swap(false, Ordering::Relaxed) {
        data.discard(usize::MAX);
        data.primed = false;
    }
    let available = data.consumer.slots();
    let max_queue = n + 2 * data.target_slack;
    if available > max_queue {
        // Too much queued (nobody was reading, or the slack just shrank):
        // skip ahead to keep the delay bounded, and fade back in over the cut.
        let excess = available - max_queue;
        data.discard(excess);
        data.fade_left = FADE_FRAMES;
        data.stats
            .trimmed_frames
            .fetch_add(excess as u64, Ordering::Relaxed);
    }
    if !data.primed {
        data.primed = data.consumer.slots() >= n + data.target_slack;
        if data.primed {
            data.fade_left = FADE_FRAMES;
        }
    }
    let have = data.primed && data.consumer.slots() >= n;
    if data.primed && !have {
        data.primed = false;
        data.calm_frames = 0;
        if data.target_slack < data.max_slack {
            data.target_slack += data.quantum;
        }
        if data.count_callbacks {
            // Counted only: the window shows dropouts, and logging (which
            // allocates and takes a lock) does not belong on the RT thread.
            data.stats.underruns.fetch_add(1, Ordering::Relaxed);
        }
    }
    if have {
        data.calm_frames += n;
        if data.calm_frames >= data.relax_after {
            data.calm_frames = 0;
            if data.target_slack > data.min_slack {
                data.target_slack -= data.quantum.min(data.target_slack - data.min_slack);
            }
        }
    }
    // Fetch the block: straight from the ring, or one sample more or fewer
    // stretched to fit when this stream follows another clock.
    let n = n.min(MAX_BLOCK);
    if have && n > 0 {
        let available = data.consumer.slots();
        let take = match &data.drift {
            Some(_) => Slip::take(available, n, n + data.target_slack, data.quantum / 2),
            None => n,
        }
        .min(available);
        for staged in data.staging[..take].iter_mut() {
            *staged = data.consumer.pop().unwrap_or(0.0);
        }
        match data.drift.as_mut() {
            Some(slip) => slip.stretch(&data.staging[..take], &mut data.block[..n]),
            None => data.block[..n].copy_from_slice(&data.staging[..n]),
        }
    }
    let frames = bytes.as_chunks_mut::<SAMPLE_BYTES>().0;
    for (frame, &sample) in frames
        .chunks_exact_mut(data.channels)
        .take(n)
        .zip(&data.block)
    {
        let mut s = if have { sample } else { 0.0 };
        if data.fade_left > 0 && have {
            s *= 1.0 - data.fade_left as f32 / FADE_FRAMES as f32;
            data.fade_left -= 1;
        }
        let le = s.to_le_bytes();
        for dst in frame {
            *dst = le;
        }
    }
    let chunk = d.chunk_mut();
    *chunk.offset_mut() = 0;
    *chunk.stride_mut() = frame_bytes as i32;
    *chunk.size_mut() = (n * frame_bytes) as u32;
}

/// Flush denormal numbers to zero on this thread. Filters and reverb tails
/// decaying into silence otherwise produce denormals, which are up to a
/// hundred times slower to compute and show up as dropouts after you stop
/// talking.
#[inline]
fn no_denormals() {
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

/// Everything a session needs, owned by the thread for its whole life.
struct LoopContext {
    config: AudioConfig,
    pipeline: Arc<Mutex<Pipeline>>,
    stats: Arc<Stats>,
    meters: Arc<Meters>,
    devices: Arc<Mutex<Vec<DeviceInfo>>>,
    capture_target: Arc<Mutex<Option<String>>>,
    active_target: Arc<Mutex<Option<String>>>,
    monitor_on: Arc<AtomicBool>,
    app_streams: Arc<Mutex<Vec<AppStream>>>,
    routing: Arc<Mutex<Routing>>,
    health: Arc<Mutex<AudioHealth>>,
    listeners: Arc<AtomicUsize>,
    quit: Arc<AtomicBool>,
    bench: Arc<Bench>,
}

fn set_health(health: &Mutex<AudioHealth>, f: impl FnOnce(&mut AudioHealth)) {
    if let Ok(mut h) = health.lock() {
        f(&mut h);
    }
}

/// Why a session ended.
enum SessionEnd {
    Quit,
    /// The connection to the daemon was lost; try again.
    Lost(String),
}

/// The thread body: run sessions until quit, reconnecting after a loss.
fn run_thread(
    ctx: LoopContext,
    receiver: pw::channel::Receiver<Command>,
    ready: std::sync::mpsc::Sender<Result<()>>,
) -> Result<()> {
    let mut receiver = Some(receiver);
    let mut ready = Some(ready);
    let mut backoff = RECONNECT_MIN;
    loop {
        let first = ready.is_some();
        let rx = receiver
            .take()
            .expect("receiver is handed back after every session");
        match run_session(&ctx, rx, ready.take()) {
            Ok((SessionEnd::Quit, _)) => return Ok(()),
            Ok((SessionEnd::Lost(why), rx)) => {
                receiver = Some(rx);
                if ctx.quit.load(Ordering::SeqCst) {
                    return Ok(());
                }
                log::warn!("audio session lost ({why}); reconnecting");
                set_health(&ctx.health, |h| {
                    h.state = AudioState::Reconnecting;
                    h.message = "Reconnecting to the audio system…".into();
                    h.virtual_mic = false;
                    h.mic_alive = false;
                });
                ctx.listeners.store(0, Ordering::Relaxed);
                backoff = RECONNECT_MIN;
            }
            Err((e, rx)) => {
                if first {
                    // The very first attempt failed: the caller shows it.
                    return Err(e);
                }
                receiver = Some(rx);
                if ctx.quit.load(Ordering::SeqCst) {
                    return Ok(());
                }
                log::warn!("could not reconnect to PipeWire ({e:#}); retrying in {backoff:?}");
                set_health(&ctx.health, |h| {
                    h.state = AudioState::Reconnecting;
                    h.message = format!("Waiting for the audio system to come back ({e:#})");
                });
            }
        }
        // Sleep in slices so Quit is honoured quickly.
        let until = Instant::now() + backoff;
        while Instant::now() < until {
            if ctx.quit.load(Ordering::SeqCst) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }
}

/// Node/port ids used to wire our processed-audio output into the virtual
/// microphone node. Linking by explicit ports is reliable where node-only
/// linking is not.
#[derive(Default)]
struct FeedLink {
    out_node: Option<u32>,
    src_node: Option<u32>,
    out_port: Option<u32>,
    in_port: Option<u32>,
    link: Option<pw::link::Link>,
}

/// One connection to the daemon. Returns how it ended plus the command
/// receiver (so the next session can reuse it), or the setup error.
#[allow(clippy::type_complexity)]
fn run_session(
    ctx: &LoopContext,
    receiver: pw::channel::Receiver<Command>,
    ready: Option<std::sync::mpsc::Sender<Result<()>>>,
) -> std::result::Result<
    (SessionEnd, pw::channel::Receiver<Command>),
    (anyhow::Error, pw::channel::Receiver<Command>),
> {
    if let Ok(mut r) = ctx.routing.lock() {
        r.reset_for_new_session();
    }
    if let Ok(mut d) = ctx.devices.lock() {
        d.clear();
    }
    if let Ok(mut a) = ctx.app_streams.lock() {
        a.clear();
    }
    set_health(&ctx.health, |h| h.sessions += 1);

    // Build everything inside a closure so that *every* failure, not only
    // the connection, is reported to whoever is waiting.
    let built = build_session(ctx);
    let session = match built {
        Ok(s) => s,
        Err(e) => {
            if let Some(ready) = ready {
                set_health(&ctx.health, |h| {
                    h.state = AudioState::Failed;
                    h.message = format!("{e:#}");
                });
                let _ = ready.send(Err(anyhow!("{e:#}")));
            }
            return Err((e, receiver));
        }
    };
    let Session {
        mainloop,
        _context,
        _core,
        _core_listener,
        _vsource,
        _capture,
        _output,
        _monitor,
        _capture_listener,
        _output_listener,
        _monitor_listener,
        _registry,
        _registry_listener,
        _metadata,
        _metadata_listener,
        _source_node,
        _feed_link,
        end,
        receiver_slot,
    } = session;

    // Commands arrive through the channel attached to this loop.
    let attached = receiver.attach(mainloop.loop_(), {
        let slot = receiver_slot.clone();
        move |cmd| {
            if let Some(handler) = slot.borrow().as_ref() {
                handler(cmd);
            }
        }
    });

    set_health(&ctx.health, |h| {
        h.state = AudioState::Running;
        h.message.clear();
    });
    if let Some(ready) = ready {
        let _ = ready.send(Ok(()));
    }
    log::info!("audio session running");
    mainloop.run();
    log::info!("PipeWire loop stopped");
    let receiver = attached.deattach();
    let outcome = end
        .take()
        .unwrap_or(SessionEnd::Lost("loop stopped".into()));
    Ok((outcome, receiver))
}

type CommandHandler = Rc<RefCell<Option<Box<dyn Fn(Command)>>>>;

/// Everything that has to stay alive while the loop runs.
struct Session {
    mainloop: pw::main_loop::MainLoopRc,
    _context: pw::context::ContextRc,
    _core: pw::core::CoreRc,
    _core_listener: pw::core::Listener,
    _vsource: pw::node::Node,
    _capture: pw::stream::StreamRc,
    _output: pw::stream::StreamRc,
    _monitor: pw::stream::StreamRc,
    _capture_listener: pw::stream::StreamListener<CaptureData>,
    _output_listener: pw::stream::StreamListener<SinkData>,
    _monitor_listener: pw::stream::StreamListener<SinkData>,
    _registry: pw::registry::RegistryRc,
    _registry_listener: pw::registry::Listener,
    _metadata: Rc<RefCell<Option<pw::metadata::Metadata>>>,
    _metadata_listener: Rc<RefCell<Option<pw::metadata::MetadataListener>>>,
    _source_node: Rc<RefCell<Option<pw::node::Node>>>,
    _feed_link: Rc<RefCell<FeedLink>>,
    end: Rc<RefCell<Option<SessionEnd>>>,
    receiver_slot: CommandHandler,
}

#[allow(clippy::too_many_lines)]
fn build_session(ctx: &LoopContext) -> Result<Session> {
    let config = &ctx.config;
    let stats = ctx.stats.clone();
    let devices = ctx.devices.clone();
    let capture_target = ctx.capture_target.clone();
    let active_target = ctx.active_target.clone();
    let app_streams = ctx.app_streams.clone();
    let routing = ctx.routing.clone();
    let health = ctx.health.clone();

    let mainloop = pw::main_loop::MainLoopRc::new(None).context("create main loop")?;
    let context = pw::context::ContextRc::new(&mainloop, None).context("create context")?;
    let core = context
        .connect_rc(None)
        .context("connect to the PipeWire audio system")?;

    let end: Rc<RefCell<Option<SessionEnd>>> = Rc::new(RefCell::new(None));
    // Ends the session (once) from any callback.
    let lose = {
        let end = end.clone();
        let mainloop = mainloop.clone();
        Rc::new(move |why: String| {
            let mut e = end.borrow_mut();
            if e.is_none() {
                *e = Some(SessionEnd::Lost(why));
            }
            // Also when a quit is waiting for its round trip: the daemon is
            // gone, nothing more will arrive.
            mainloop.quit();
        })
    };
    // The sequence number of the round trip a quit waits for (see `Command::Quit`).
    let quit_seq: Rc<Cell<Option<i32>>> = Rc::new(Cell::new(None));

    let latency = format!("{}/{}", config.quantum, config.rate);
    // One scheduling group (same driver, same cycle) and one link group (the
    // session manager never links us to ourselves) for all our nodes.
    let group = format!(
        "voice-changer.{}.{}",
        std::process::id(),
        config.source_name
    );

    // The virtual microphone apps capture from is a `support.null-audio-sink`
    // node published as a virtual source. Unlike a plain pw_stream source it
    // is a real driver, so PulseAudio clients (Discord, Chromium, OBS) can
    // negotiate with it -- a pw_stream source leaves them stuck in
    // "negotiating" and silent. We feed it from our output stream below.
    let vsource_props = properties! {
        "factory.name" => "support.null-audio-sink",
        *pw::keys::NODE_NAME => config.source_name.as_str(),
        *pw::keys::NODE_DESCRIPTION => SOURCE_DESCRIPTION,
        *pw::keys::MEDIA_CLASS => "Audio/Source/Virtual",
        *pw::keys::NODE_VIRTUAL => "true",
        *pw::keys::AUDIO_CHANNELS => "1",
        "audio.position" => "[MONO]",
        "node.group" => group.as_str(),
        "node.link-group" => group.as_str(),
        "object.linger" => "false",
    };
    let vsource: pw::node::Node = core
        .create_object("adapter", &vsource_props)
        .context("create the virtual microphone")?;
    let vsource_id = vsource.upcast_ref().id();

    // Errors from the daemon arrive here: a failed object creation names
    // the proxy, a dead connection names the core (id 0).
    let core_listener = core
        .add_listener_local()
        .done({
            let quit_seq = quit_seq.clone();
            let mainloop = mainloop.clone();
            move |id, seq| {
                if id == pw::core::PW_ID_CORE && quit_seq.get() == Some(seq.seq()) {
                    mainloop.quit();
                }
            }
        })
        .error({
            let lose = lose.clone();
            let health = health.clone();
            move |id, _seq, res, message| {
                if id == 0 || res == -libc_epipe() {
                    log::error!("PipeWire connection error: {message} ({res})");
                    lose(message.to_string());
                } else if id == vsource_id {
                    log::error!("the virtual microphone could not be created: {message} ({res})");
                    set_health(&health, |h| {
                        h.virtual_mic = false;
                        h.message = format!(
                            "The audio system refused to create the virtual microphone ({message}). Apps cannot pick “Voice Changer Mic”; Hear myself still works."
                        );
                    });
                } else {
                    log::warn!("PipeWire error on object {id}: {message} ({res})");
                }
            }
        })
        .register();

    // One second of headroom each; the sink callbacks trim their queues so
    // real latency stays around one quantum.
    let (to_source, from_capture) = rtrb::RingBuffer::<f32>::new(config.rate as usize);
    let (to_monitor, from_capture_mon) = rtrb::RingBuffer::<f32>::new(config.rate as usize);

    // ---- capture: our microphone input (processing happens here) ---------------------
    let mut cap_props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Communication",
        *pw::keys::NODE_NAME => CAPTURE_NODE_NAME,
        *pw::keys::NODE_DESCRIPTION => "Voice Changer (mic input)",
        *pw::keys::NODE_LATENCY => latency.as_str(),
        // Stereo, folded down by `StereoFold`: a mono microphone arrives on
        // both channels, an interface with the microphone on one input is
        // not halved the way the graph's own stereo-to-mono mix would.
        *pw::keys::AUDIO_CHANNELS => "2",
        "audio.position" => "[FL,FR]",
        "node.group" => group.as_str(),
        "node.link-group" => group.as_str(),
    };
    let initial_target = active_target.lock().ok().and_then(|t| t.clone());
    if let Some(target) = &initial_target {
        cap_props.insert(*pw::keys::TARGET_OBJECT, target.as_str());
    }
    let capture = pw::stream::StreamRc::new(core.clone(), "Voice Changer capture", cap_props)
        .context("create capture stream")?;
    let capture_listener = capture
        .add_local_listener_with_user_data(CaptureData {
            to_source,
            to_monitor,
            monitor_on: ctx.monitor_on.clone(),
            format: Default::default(),
            fold: StereoFold::new(config.rate as f32),
            left: vec![0.0; MAX_BLOCK],
            right: vec![0.0; MAX_BLOCK],
            pipeline: ctx.pipeline.clone(),
            scratch: vec![0.0; MAX_BLOCK],
            input_wav: config.input_wav.clone(),
            wav_pos: 0,
            bench: ctx.bench.clone(),
            bench_pos: 0,
            stats: stats.clone(),
            meters: ctx.meters.clone(),
        })
        .state_changed({
            let lose = lose.clone();
            move |_, _, old, new| {
                log::info!("capture stream: {old:?} -> {new:?}");
                if let pw::stream::StreamState::Error(e) = new {
                    lose(format!("capture stream error: {e}"));
                }
            }
        })
        .param_changed(|_, data, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((mt, mst)) = format_utils::parse_format(param) else {
                return;
            };
            if mt != MediaType::Audio || mst != MediaSubtype::Raw {
                return;
            }
            if data.format.parse(param).is_ok() {
                // A new format means a new microphone: learn its channels afresh.
                data.fold.reset();
                log::info!(
                    "capture format: {:?} {} Hz x{}",
                    data.format.format(),
                    data.format.rate(),
                    data.format.channels()
                );
            }
        })
        .process(|stream, data| {
            no_denormals();
            data.stats.capture_callbacks.fetch_add(1, Ordering::Relaxed);
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            let Some(d) = datas.first_mut() else { return };
            let size = d.chunk().size() as usize;
            let offset = d.chunk().offset() as usize;
            let channels = data.format.channels().max(1) as usize;
            let Some(bytes) = d.data() else { return };
            // A chunk that points past its buffer must not panic the RT thread.
            let offset = offset.min(bytes.len());
            let end = (offset + size).min(bytes.len());
            let frame_bytes = SAMPLE_BYTES * channels;
            let frames = ((end - offset) / frame_bytes).min(data.scratch.len());
            let scratch = &mut data.scratch[..frames];
            let interleaved = bytes[offset..end].chunks_exact(frame_bytes);
            if channels == 2 {
                let (left, right) = (&mut data.left[..frames], &mut data.right[..frames]);
                for ((l, r), frame) in left.iter_mut().zip(right.iter_mut()).zip(interleaved) {
                    let (samples, _) = frame.as_chunks::<SAMPLE_BYTES>();
                    *l = f32::from_le_bytes(samples[0]);
                    *r = f32::from_le_bytes(samples[1]);
                }
                data.fold.process(left, right, scratch);
            } else {
                // Whatever else was negotiated: a plain average.
                for (out, frame) in scratch.iter_mut().zip(interleaved) {
                    let mut acc = 0.0f32;
                    for s in frame.as_chunks::<SAMPLE_BYTES>().0 {
                        acc += f32::from_le_bytes(*s);
                    }
                    *out = acc / channels as f32;
                }
            }
            if let Some(wav) = &data.input_wav
                && !wav.is_empty()
            {
                for out in scratch.iter_mut() {
                    *out = wav[data.wav_pos];
                    data.wav_pos = (data.wav_pos + 1) % wav.len();
                }
            }

            // Test bench: keep the raw microphone if a recording runs, and
            // replay a clip in its place if one is playing.
            data.bench.capture(scratch);
            let bench_clip = data.bench.play.load();
            let benching = bench_clip.is_some();
            if let Some(clip) = bench_clip.as_ref() {
                for out in scratch.iter_mut() {
                    data.bench_pos %= clip.len();
                    *out = clip[data.bench_pos];
                    data.bench_pos += 1;
                }
            }
            drop(bench_clip);

            // Uncontended except for the instant a session is torn down.
            let Ok(mut pipeline) = data.pipeline.try_lock() else {
                data.stats.skipped_callbacks.fetch_add(1, Ordering::Relaxed);
                return;
            };
            let started = std::time::Instant::now();
            pipeline.process(scratch);
            let us = started.elapsed().as_micros() as u64;
            data.stats.process_max_us.fetch_max(us, Ordering::Relaxed);
            let avg = data.stats.process_avg_us.load(Ordering::Relaxed);
            data.stats
                .process_avg_us
                .store((avg * 15 + us) / 16, Ordering::Relaxed);
            let (i, o) = pipeline.peaks();
            data.meters.publish(i, o, pipeline.engine.vad() > 0.5);
            data.meters.publish_auto_gain(pipeline.auto_gain_db());
            drop(pipeline);

            // The monitor stream only exists while "Hear myself" is on.
            if data.monitor_on.load(Ordering::Relaxed) {
                push_all(&mut data.to_monitor, scratch, None);
            }
            // A bench clip is for the user's ears only: the virtual
            // microphone carries silence while one plays.
            if benching {
                scratch.fill(0.0);
            }
            push_all(&mut data.to_source, scratch, Some(&data.stats));
        })
        .register()
        .context("register capture listener")?;

    // ---- output: feeds the virtual microphone ----------------------------------------
    let out_props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Playback",
        *pw::keys::MEDIA_ROLE => "Communication",
        *pw::keys::NODE_NAME => OUTPUT_NODE_NAME,
        *pw::keys::NODE_DESCRIPTION => "Voice Changer (processed voice)",
        *pw::keys::NODE_LATENCY => latency.as_str(),
        *pw::keys::AUDIO_CHANNELS => "1",
        "audio.position" => "[MONO]",
        "node.group" => group.as_str(),
        "node.link-group" => group.as_str(),
    };
    let output = pw::stream::StreamRc::new(core.clone(), "Voice Changer output", out_props)
        .context("create output stream")?;
    let output_listener = output
        .add_local_listener_with_user_data(SinkData::new(
            from_capture,
            1,
            config,
            stats.clone(),
            true,
            false,
        ))
        .state_changed({
            let lose = lose.clone();
            move |_, _, old, new| {
                log::info!("source stream: {old:?} -> {new:?}");
                if let pw::stream::StreamState::Error(e) = new {
                    lose(format!("output stream error: {e}"));
                }
            }
        })
        .process(drain_into)
        .register()
        .context("register output listener")?;

    // ---- monitor: hear yourself through the default output -------------------------------
    // Role "Music" rather than "Communication": a communication stream
    // makes WirePlumber switch Bluetooth headsets into the low-quality
    // headset profile even while this stream only carries silence.
    // Stereo with the voice on both channels, so it is in both ears and in
    // the front pair of a surround output whatever the graph's up-mix
    // settings are.
    let mut mon_props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Playback",
        *pw::keys::MEDIA_ROLE => "Music",
        *pw::keys::NODE_NAME => MONITOR_NODE_NAME,
        *pw::keys::NODE_DESCRIPTION => "Voice Changer monitor",
        *pw::keys::NODE_LATENCY => latency.as_str(),
        *pw::keys::AUDIO_CHANNELS => "2",
        "audio.position" => "[FL,FR]",
        // Deliberately not in our scheduling group: that would put the
        // output device, and with it every application playing sound, on
        // the microphone's clock, so a late microphone cycle would be heard
        // in everything. The monitor stays on the output device's clock and
        // absorbs the drift between the two itself (see `slip.rs`).
        "node.link-group" => group.as_str(),
    };
    if let Some(target) = &config.monitor_target {
        mon_props.insert(*pw::keys::TARGET_OBJECT, target.as_str());
    }
    let monitor = pw::stream::StreamRc::new(core.clone(), "Voice Changer monitor", mon_props)
        .context("create monitor stream")?;
    let monitor_data = SinkData::new(from_capture_mon, 2, config, stats.clone(), false, true);
    let monitor_flush = monitor_data.flush.clone();
    let monitor_listener = monitor
        .add_local_listener_with_user_data(monitor_data)
        .state_changed(|_, _, old, new| log::info!("monitor stream: {old:?} -> {new:?}"))
        .process(drain_into)
        .register()
        .context("register monitor listener")?;

    let format = f32_format(config.rate, 1)?;
    let stereo = f32_format(config.rate, 2)?;
    let mut cap_params = [Pod::from_bytes(&stereo).ok_or_else(|| anyhow!("bad format pod"))?];
    capture
        .connect(
            spa::utils::Direction::Input,
            None,
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
            &mut cap_params,
        )
        .context("connect capture stream")?;
    let mut out_params = [Pod::from_bytes(&format).ok_or_else(|| anyhow!("bad format pod"))?];
    output
        .connect(
            spa::utils::Direction::Output,
            None,
            StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
            &mut out_params,
        )
        .context("connect output stream")?;
    // The monitor is connected only while "Hear myself" is on: a stream
    // that is always there keeps the headphones awake, sits in every volume
    // mixer and ties the output device into our graph for nothing.
    if ctx.monitor_on.load(Ordering::Relaxed) {
        set_monitor_stream(&monitor, true, config.rate, &monitor_flush);
    }

    // ---- registry: microphone list + the "default" metadata object -------------------
    let registry = core.get_registry_rc().context("get registry")?;
    let metadata: Rc<RefCell<Option<pw::metadata::Metadata>>> = Rc::new(RefCell::new(None));
    let metadata_listener: Rc<RefCell<Option<pw::metadata::MetadataListener>>> =
        Rc::new(RefCell::new(None));
    let source_node: Rc<RefCell<Option<pw::node::Node>>> = Rc::new(RefCell::new(None));
    // Tracks the node/port ids needed to link our output into the virtual mic.
    let feed_link: Rc<RefCell<FeedLink>> = Rc::new(RefCell::new(FeedLink::default()));
    // Links from the virtual mic's output to other nodes (= apps listening).
    let listener_links: Rc<RefCell<HashSet<u32>>> = Rc::new(RefCell::new(HashSet::new()));
    // The take-over waits here until the session manager's metadata and a
    // real microphone are known (see `take_over_timer`).
    let take_over_pending: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let source_name = config.source_name.clone();

    let registry_listener = registry
        .add_listener_local()
        .global({
            let devices = devices.clone();
            let app_streams = app_streams.clone();
            let routing = routing.clone();
            let metadata = metadata.clone();
            let metadata_listener = metadata_listener.clone();
            let source_node = source_node.clone();
            let feed_link = feed_link.clone();
            let listener_links = listener_links.clone();
            let listeners = ctx.listeners.clone();
            let health = health.clone();
            let core = core.clone();
            let registry = registry.downgrade();
            let source_name = source_name.clone();
            let capture = capture.clone();
            let capture_target = capture_target.clone();
            let active_target = active_target.clone();
            let take_over_pending = take_over_pending.clone();
            move |global| match global.type_ {
                ObjectType::Node => {
                    if let Some(dev) = device_from_global(global) {
                        let name = dev.name.clone();
                        if let Ok(mut list) = devices.lock() {
                            list.retain(|d| d.id != dev.id);
                            list.push(dev);
                            list.sort_by(|a, b| a.description.cmp(&b.description));
                        }
                        // The microphone the user chose came back: use it again.
                        let preferred = capture_target.lock().ok().and_then(|t| t.clone());
                        let active = active_target.lock().ok().and_then(|t| t.clone());
                        if preferred.as_deref() == Some(name.as_str()) && active != preferred {
                            log::info!("microphone {name:?} is back; switching to it");
                            if let Ok(mut a) = active_target.lock() {
                                *a = preferred.clone();
                            }
                            retarget_capture(
                                &capture,
                                metadata.borrow().as_ref(),
                                &devices,
                                Some(&name),
                            );
                        }
                    }
                    let props = global.props.as_ref();
                    let node_name = props.and_then(|p| p.get("node.name"));
                    if node_name == Some(source_name.as_str()) {
                        if let Some(serial) = props.and_then(|p| p.get("object.serial"))
                            && let Ok(mut r) = routing.lock()
                        {
                            r.source_serial = Some(serial.to_string());
                        }
                        if let Some(registry) = registry.upgrade() {
                            match registry.bind::<pw::node::Node, _>(global) {
                                Ok(node) => {
                                    set_unity_volume(&node);
                                    *source_node.borrow_mut() = Some(node);
                                }
                                Err(e) => log::warn!("could not bind the virtual mic node: {e}"),
                            }
                        }
                    }
                    // Record the node ids so we can link our output's port into
                    // the virtual mic's input port once the ports appear.
                    if node_name == Some(OUTPUT_NODE_NAME) {
                        feed_link.borrow_mut().out_node = Some(global.id);
                    } else if node_name == Some(source_name.as_str()) {
                        feed_link.borrow_mut().src_node = Some(global.id);
                    }
                    try_feed_link(&core, &feed_link, &health);
                    if let Some(app) = app_stream_from_global(global) {
                        let id = app.id;
                        if let Ok(mut list) = app_streams.lock() {
                            list.retain(|a| a.id != id);
                            list.push(app);
                        }
                        // An app that opens while we are the microphone for
                        // everything is routed to us too.
                        if routing
                            .lock()
                            .map(|r| {
                                r.take_over
                                    && r.default_is_us
                                    && !r.original_targets.contains_key(&id)
                            })
                            .unwrap_or(false)
                        {
                            route_stream(
                                metadata.borrow().as_ref(),
                                &routing,
                                &source_name,
                                id,
                                true,
                            );
                        }
                    }
                }
                ObjectType::Port => {
                    let props = global.props.as_ref();
                    let pnode: Option<u32> = props
                        .and_then(|p| p.get("node.id"))
                        .and_then(|s| s.parse().ok());
                    let dir = props.and_then(|p| p.get("port.direction"));
                    let mut fl = feed_link.borrow_mut();
                    if pnode == fl.out_node && dir == Some("out") {
                        fl.out_port = Some(global.id);
                    } else if pnode == fl.src_node && dir == Some("in") {
                        fl.in_port = Some(global.id);
                    }
                    drop(fl);
                    try_feed_link(&core, &feed_link, &health);
                }
                ObjectType::Link => {
                    // A link *from* the virtual mic is an application hearing us.
                    let props = global.props.as_ref();
                    let out_node: Option<u32> = props
                        .and_then(|p| p.get("link.output.node"))
                        .and_then(|s| s.parse().ok());
                    let src = feed_link.borrow().src_node;
                    if out_node.is_some() && out_node == src {
                        listener_links.borrow_mut().insert(global.id);
                        listeners.store(listener_links.borrow().len(), Ordering::Relaxed);
                    }
                }
                ObjectType::Metadata => {
                    let is_default = global
                        .props
                        .as_ref()
                        .and_then(|p| p.get("metadata.name"))
                        .is_some_and(|n| n == "default");
                    if is_default && let Some(registry) = registry.upgrade() {
                        match registry.bind::<pw::metadata::Metadata, _>(global) {
                            Ok(m) => {
                                let routing_for_props = routing.clone();
                                let listener = m
                                    .add_listener_local()
                                    .property(move |subject, key, _type, value| {
                                        if let Ok(mut r) = routing_for_props.lock() {
                                            match key {
                                                Some("default.configured.audio.source")
                                                    if subject == 0 =>
                                                {
                                                    r.default_current = value.map(str::to_string);
                                                }
                                                Some("target.object") => match value {
                                                    Some(v) => {
                                                        r.targets.insert(subject, v.to_string());
                                                    }
                                                    None => {
                                                        r.targets.remove(&subject);
                                                    }
                                                },
                                                _ => {}
                                            }
                                        }
                                        0
                                    })
                                    .register();
                                *metadata_listener.borrow_mut() = Some(listener);
                                *metadata.borrow_mut() = Some(m);
                                // A remembered take-over can now be applied
                                // (the timer waits for a real microphone too).
                                if routing.lock().map(|r| r.take_over).unwrap_or(false) {
                                    take_over_pending.set(true);
                                }
                            }
                            Err(e) => log::warn!("could not bind default metadata: {e}"),
                        }
                    }
                }
                _ => {}
            }
        })
        .global_remove({
            let devices = devices.clone();
            let app_streams = app_streams.clone();
            let routing = routing.clone();
            let listener_links = listener_links.clone();
            let listeners = ctx.listeners.clone();
            let active_target = active_target.clone();
            let capture = capture.clone();
            let metadata = metadata.clone();
            let source_name = source_name.clone();
            move |id| {
                let mut unplugged: Option<DeviceInfo> = None;
                if let Ok(mut list) = devices.lock()
                    && let Some(pos) = list.iter().position(|d| d.id == id)
                {
                    unplugged = Some(list.remove(pos));
                }
                if let Ok(mut list) = app_streams.lock() {
                    list.retain(|a| a.id != id);
                }
                if let Ok(mut r) = routing.lock() {
                    r.targets.remove(&id);
                    r.original_targets.remove(&id);
                }
                if listener_links.borrow_mut().remove(&id) {
                    listeners.store(listener_links.borrow().len(), Ordering::Relaxed);
                }
                // The microphone in use vanished: fall back to the default
                // for now; the registry handler switches back when it returns.
                if let Some(dev) = unplugged {
                    let active = active_target.lock().ok().and_then(|t| t.clone());
                    if active.as_deref() == Some(dev.name.as_str()) {
                        log::warn!(
                            "microphone {:?} was unplugged; using the system default until it is back",
                            dev.description
                        );
                        // While we are the default source "the default"
                        // would be ourselves: name another real microphone.
                        let fallback = if routing.lock().map(|r| r.default_is_us).unwrap_or(false) {
                            real_default_source(&routing, &devices, &source_name)
                        } else {
                            None
                        };
                        if let Ok(mut a) = active_target.lock() {
                            *a = fallback.clone();
                        }
                        retarget_capture(
                            &capture,
                            metadata.borrow().as_ref(),
                            &devices,
                            fallback.as_deref(),
                        );
                    }
                }
            }
        })
        .register();

    // ---- commands from the rest of the app ------------------------------------------------
    let receiver_slot: CommandHandler = Rc::new(RefCell::new(None));
    {
        let mainloop = mainloop.clone();
        let devices = devices.clone();
        let metadata = metadata.clone();
        let capture = capture.clone();
        let monitor = monitor.clone();
        let routing = routing.clone();
        let source_name = source_name.clone();
        let capture_target = capture_target.clone();
        let active_target = active_target.clone();
        let app_streams = app_streams.clone();
        let end = end.clone();
        let take_over_pending = take_over_pending.clone();
        let core = core.clone();
        let quit_seq = quit_seq.clone();
        let rate = config.rate;
        let handler: Box<dyn Fn(Command)> = Box::new(move |cmd| match cmd {
            Command::Quit => {
                // Put every application back on its own microphone, then
                // restore the default, before leaving.
                if routing.lock().map(|r| r.take_over).unwrap_or(false) {
                    restore_all_apps(metadata.borrow().as_ref(), &routing);
                }
                set_default_source(metadata.borrow().as_ref(), &routing, &source_name, false);
                *end.borrow_mut() = Some(SessionEnd::Quit);
                // The restores above are only queued. Leaving now would
                // drop them with the connection and leave the desktop's
                // default microphone pointing at a node that is gone, so
                // wait for the daemon to answer a round trip first (a timer
                // below gives up after a second if it never does).
                match core.sync(0) {
                    Ok(seq) => quit_seq.set(Some(seq.seq())),
                    Err(_) => mainloop.quit(),
                }
            }
            Command::RouteApp { stream_id, on } => {
                route_stream(
                    metadata.borrow().as_ref(),
                    &routing,
                    &source_name,
                    stream_id,
                    on,
                );
            }
            Command::SetDefaultSource(on) => {
                if on {
                    if let Ok(mut r) = routing.lock() {
                        r.take_over = true;
                    }
                    let applied = apply_take_over(
                        &capture,
                        metadata.borrow().as_ref(),
                        &devices,
                        &routing,
                        &source_name,
                        &app_streams,
                        &capture_target,
                    );
                    // Not possible yet (no session manager / no microphone
                    // known): the timer below keeps trying.
                    take_over_pending.set(!applied);
                } else {
                    take_over_pending.set(false);
                    if let Ok(mut r) = routing.lock() {
                        r.take_over = false;
                    }
                    restore_all_apps(metadata.borrow().as_ref(), &routing);
                    set_default_source(metadata.borrow().as_ref(), &routing, &source_name, false);
                    let want = capture_target.lock().ok().and_then(|t| t.clone());
                    if let Ok(mut a) = active_target.lock() {
                        *a = want.clone();
                    }
                    retarget_capture(
                        &capture,
                        metadata.borrow().as_ref(),
                        &devices,
                        want.as_deref(),
                    );
                }
            }
            Command::SetCaptureTarget(target) => {
                if let Ok(mut a) = active_target.lock() {
                    *a = target.clone();
                }
                retarget_capture(
                    &capture,
                    metadata.borrow().as_ref(),
                    &devices,
                    target.as_deref(),
                );
            }
            Command::SetMonitor(on) => {
                set_monitor_stream(&monitor, on, rate, &monitor_flush);
            }
        });
        *receiver_slot.borrow_mut() = Some(handler);
    }
    log::info!(
        "capture target: {}",
        initial_target
            .clone()
            .unwrap_or_else(|| "system default".into())
    );

    // A saved microphone that no longer exists (unplugged, renamed) must not
    // leave the capture stream pointing at nothing: fall back to the default
    // (and come back to it when it reappears, see the registry handler).
    if let Some(wanted) = initial_target.clone() {
        let devices = devices.clone();
        let active_target = active_target.clone();
        let capture = capture.clone();
        let metadata = metadata.clone();
        let timer = mainloop.loop_().add_timer(move |_| {
            let known = devices
                .lock()
                .map(|d| d.iter().any(|x| x.name == wanted))
                .unwrap_or(true);
            if !known {
                log::warn!("saved microphone {wanted:?} is not present; using the system default");
                if let Ok(mut t) = active_target.lock() {
                    *t = None;
                }
                retarget_capture(&capture, metadata.borrow().as_ref(), &devices, None);
            }
        });
        let _ = timer.update_timer(Some(Duration::from_millis(1500)), None);
        // SAFETY of lifetime: the timer borrows the loop, which lives in the
        // same `Session`; leaking it is simpler than carrying the borrow.
        std::mem::forget(timer);
    }

    // Keep the virtual mic at unity. The session manager restores whatever
    // level it remembered for our node (one user's was at 16%) a moment after
    // the node appears, so set it on the node once it is bound (registry
    // handler) and re-assert it now and then.
    {
        let source_node = source_node.clone();
        let timer = mainloop.loop_().add_timer(move |_| {
            if let Some(node) = source_node.borrow().as_ref() {
                set_unity_volume(node);
            }
        });
        let _ = timer.update_timer(
            Some(Duration::from_millis(1500)),
            Some(Duration::from_secs(5)),
        );
        std::mem::forget(timer);
    }

    // Take-over that could not be applied immediately (the session manager
    // or a real microphone was not known yet, or we just reconnected):
    // retry every half second until it sticks.
    {
        let take_over_pending = take_over_pending.clone();
        let capture = capture.clone();
        let metadata = metadata.clone();
        let devices = devices.clone();
        let routing = routing.clone();
        let source_name = source_name.clone();
        let app_streams = app_streams.clone();
        let capture_target = capture_target.clone();
        let end = end.clone();
        let quit_loop = mainloop.clone();
        let quit_ticks = Cell::new(0u32);
        let metadata_ticks = Cell::new(0u32);
        let timer = mainloop.loop_().add_timer(move |_| {
            // A quit whose round trip never came back.
            if matches!(*end.borrow(), Some(SessionEnd::Quit)) {
                quit_ticks.set(quit_ticks.get() + 1);
                if quit_ticks.get() >= 2 {
                    quit_loop.quit();
                }
                return;
            }
            // Once the session manager's values are in: undo a take-over
            // that a crash or a kill left behind.
            if metadata.borrow().is_some() && metadata_ticks.get() < 2 {
                metadata_ticks.set(metadata_ticks.get() + 1);
                if metadata_ticks.get() == 2 {
                    recover_default_source(metadata.borrow().as_ref(), &routing, &source_name);
                }
            }
            if !take_over_pending.get() {
                return;
            }
            if !routing.lock().map(|r| r.take_over).unwrap_or(false) {
                take_over_pending.set(false);
                return;
            }
            if apply_take_over(
                &capture,
                metadata.borrow().as_ref(),
                &devices,
                &routing,
                &source_name,
                &app_streams,
                &capture_target,
            ) {
                take_over_pending.set(false);
            }
        });
        let _ = timer.update_timer(
            Some(Duration::from_millis(500)),
            Some(Duration::from_millis(500)),
        );
        std::mem::forget(timer);
    }

    // Health: is the microphone delivering audio? (Checked every second.)
    {
        let stats = stats.clone();
        let health = health.clone();
        let last = Cell::new(0u64);
        let since = Cell::new(0u32);
        let timer = mainloop.loop_().add_timer(move |_| {
            let now = stats.capture_callbacks.load(Ordering::Relaxed);
            let alive = now != last.get();
            last.set(now);
            if alive {
                since.set(0);
            } else {
                since.set(since.get().saturating_add(1));
            }
            // Two quiet seconds in a row: the capture has no source.
            let mic_alive = alive || since.get() < 2;
            set_health(&health, |h| h.mic_alive = mic_alive);
        });
        let _ = timer.update_timer(Some(Duration::from_secs(1)), Some(Duration::from_secs(1)));
        std::mem::forget(timer);
    }
    // Re-apply a remembered take-over on every (re)connect once the
    // metadata object is bound; the registry handler flips the flag.
    if routing.lock().map(|r| r.take_over).unwrap_or(false) {
        take_over_pending.set(true);
    }

    Ok(Session {
        mainloop,
        _context: context,
        _core: core,
        _core_listener: core_listener,
        _vsource: vsource,
        _capture: capture,
        _output: output,
        _monitor: monitor,
        _capture_listener: capture_listener,
        _output_listener: output_listener,
        _monitor_listener: monitor_listener,
        _registry: registry,
        _registry_listener: registry_listener,
        _metadata: metadata,
        _metadata_listener: metadata_listener,
        _source_node: source_node,
        _feed_link: feed_link,
        end,
        receiver_slot,
    })
}

/// `EPIPE` as PipeWire reports it (negative errno), without pulling in libc.
const fn libc_epipe() -> i32 {
    32
}

/// Become the microphone for everything, if the pieces are in place.
/// Returns false when it has to wait (no session manager yet, no real
/// microphone known yet), in which case the caller retries later.
fn apply_take_over(
    capture: &pw::stream::Stream,
    metadata: Option<&pw::metadata::Metadata>,
    devices: &Mutex<Vec<DeviceInfo>>,
    routing: &Mutex<Routing>,
    source_name: &str,
    app_streams: &Mutex<Vec<AppStream>>,
    capture_target: &Mutex<Option<String>>,
) -> bool {
    let Some(md) = metadata else {
        log::info!("take-over waits for the session manager");
        return false;
    };
    if capture.node_id() == u32::MAX {
        return false;
    }
    // Our own capture follows the system default source. If we become the
    // default while it still follows, WirePlumber routes our capture onto
    // our own output -- a feedback loop that silences the real microphone
    // (and therefore Discord, routed apps and hear-myself). Pin the capture
    // to a real microphone first: the user's choice, else the current default.
    let chosen = capture_target.lock().ok().and_then(|t| t.clone());
    let present = |name: &str| {
        devices
            .lock()
            .map(|d| d.iter().any(|x| x.name == name))
            .unwrap_or(false)
    };
    let pin = chosen
        .filter(|n| present(n))
        .or_else(|| real_default_source(routing, devices, source_name));
    let Some(pin) = pin else {
        log::info!("take-over waits for a microphone to appear");
        return false;
    };
    retarget_capture(capture, Some(md), devices, Some(&pin));
    set_default_source(Some(md), routing, source_name, true);
    // Move every application that is recording right now onto the voice
    // changer (default-following apps AND apps pinned to a specific device),
    // and remember where each one was.
    let ids: Vec<u32> = app_streams
        .lock()
        .map(|l| l.iter().map(|a| a.id).collect())
        .unwrap_or_default();
    for id in ids {
        route_stream(Some(md), routing, source_name, id, true);
    }
    true
}

/// Create the output -> virtual-mic link once both ports are known.
fn try_feed_link(
    core: &pw::core::CoreRc,
    feed_link: &Rc<RefCell<FeedLink>>,
    health: &Mutex<AudioHealth>,
) {
    let mut fl = feed_link.borrow_mut();
    if fl.link.is_some() {
        return;
    }
    let (Some(on), Some(sn), Some(op), Some(ip)) =
        (fl.out_node, fl.src_node, fl.out_port, fl.in_port)
    else {
        return;
    };
    let props = properties! {
        "link.output.node" => on.to_string().as_str(),
        "link.output.port" => op.to_string().as_str(),
        "link.input.node" => sn.to_string().as_str(),
        "link.input.port" => ip.to_string().as_str(),
        "object.linger" => "false",
    };
    match core.create_object::<pw::link::Link>("link-factory", &props) {
        Ok(link) => {
            log::info!("virtual microphone connected to the processed voice");
            fl.link = Some(link);
            set_health(health, |h| {
                h.virtual_mic = true;
                if h.message.contains("virtual microphone") {
                    h.message.clear();
                }
            });
        }
        Err(e) => {
            log::warn!("could not link output to virtual mic: {e}");
            set_health(health, |h| {
                h.virtual_mic = false;
                h.message = format!("The virtual microphone could not be connected ({e}).");
            });
        }
    }
}

/// Set a node's volume to 100% (channel volumes, master volume, unmuted).
fn set_unity_volume(node: &pw::node::Node) {
    use spa::pod::{Object, Property, PropertyFlags, Value, ValueArray};
    let props = Value::Object(Object {
        type_: spa::sys::SPA_TYPE_OBJECT_Props,
        id: spa::sys::SPA_PARAM_Props,
        properties: vec![
            Property {
                key: spa::sys::SPA_PROP_volume,
                flags: PropertyFlags::empty(),
                value: Value::Float(1.0),
            },
            Property {
                key: spa::sys::SPA_PROP_mute,
                flags: PropertyFlags::empty(),
                value: Value::Bool(false),
            },
            Property {
                key: spa::sys::SPA_PROP_channelVolumes,
                flags: PropertyFlags::empty(),
                value: Value::ValueArray(ValueArray::Float(vec![1.0])),
            },
        ],
    });
    match spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &props) {
        Ok((cursor, _)) => {
            let bytes = cursor.into_inner();
            if let Some(pod) = Pod::from_bytes(&bytes) {
                node.set_param(spa::param::ParamType::Props, 0, pod);
            }
        }
        Err(e) => log::debug!("could not build the volume pod: {e:?}"),
    }
}

/// Queue a block for one of the sink callbacks. A full ring (nobody has been
/// reading for a second) drops the block; the reader trims anyway.
fn push_all(ring: &mut rtrb::Producer<f32>, block: &[f32], stats: Option<&Stats>) {
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

/// Connect or disconnect the "Hear myself" stream.
fn set_monitor_stream(monitor: &pw::stream::Stream, on: bool, rate: u32, flush: &AtomicBool) {
    use pw::stream::StreamState;
    let connected = !matches!(
        monitor.state(),
        StreamState::Unconnected | StreamState::Error(_)
    );
    if on == connected {
        return;
    }
    if !on {
        if let Err(e) = monitor.disconnect() {
            log::warn!("could not stop the monitor: {e}");
        }
        log::info!("monitor off");
        return;
    }
    // A stream that ended in an error has to be disconnected before it can
    // be connected again.
    if matches!(monitor.state(), StreamState::Error(_)) {
        let _ = monitor.disconnect();
    }
    let Some(format) = f32_format(rate, 2).ok() else {
        return;
    };
    let Some(pod) = Pod::from_bytes(&format) else {
        return;
    };
    // Whatever the ring still holds is from the last time it was on.
    flush.store(true, Ordering::Relaxed);
    match monitor.connect(
        spa::utils::Direction::Output,
        None,
        StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
        &mut [pod],
    ) {
        Ok(()) => log::info!("monitor on"),
        Err(e) => log::warn!("could not start the monitor: {e}"),
    }
}

/// Serialize an `EnumFormat` pod for 32-bit float at `rate`: mono, or stereo
/// front left/right.
fn f32_format(rate: u32, channels: u32) -> Result<Vec<u8>> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(rate);
    info.set_channels(channels);
    let mut position = [0u32; spa::param::audio::MAX_CHANNELS];
    if channels == 2 {
        position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
        position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    } else {
        position[0] = spa::sys::SPA_AUDIO_CHANNEL_MONO;
    }
    info.set_position(position);

    let (cursor, _) = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(spa::pod::Object {
            type_: spa::sys::SPA_TYPE_OBJECT_Format,
            id: spa::sys::SPA_PARAM_EnumFormat,
            properties: info.into(),
        }),
    )
    .map_err(|e| anyhow!("serialize audio format: {e:?}"))?;
    Ok(cursor.into_inner())
}

/// An application's recording stream, if this node is one (and not ours).
fn app_stream_from_global(global: &GlobalObject<&spa::utils::dict::DictRef>) -> Option<AppStream> {
    let props = global.props.as_ref()?;
    if props.get("media.class")? != "Stream/Input/Audio" {
        return None;
    }
    let node_name = props.get("node.name").unwrap_or("");
    // Ours, and PipeWire's own loopback/filter helpers, are not applications.
    if node_name.starts_with("voice_changer")
        || node_name.starts_with("vc_")
        || node_name.starts_with("input.loopback")
        || node_name.starts_with("input.filter")
        || props.get("application.name") == Some("Voice Changer")
    {
        return None;
    }
    // Electron apps (Discord, browsers) all call their stream "WEBRTC
    // VoiceEngine"; the process name says which one it really is.
    let app = props
        .get("application.name")
        .filter(|n| !n.starts_with("WEBRTC"))
        .or_else(|| props.get("application.process.binary"))
        .or_else(|| props.get("application.name"))
        .unwrap_or(node_name)
        .to_string();
    if app.is_empty() {
        return None;
    }
    Some(AppStream {
        id: global.id,
        serial: props.get("object.serial").map(str::to_string),
        app,
        media: props.get("media.name").unwrap_or("").to_string(),
        routed: false,
    })
}

/// Extract the `name` field from a `{"name":"..."}` metadata value.
fn json_name(json: &str) -> Option<String> {
    let after = &json[json.find("\"name\"")? + 6..];
    let start = after.find('"')? + 1;
    let rest = &after[start..];
    Some(rest[..rest.find('"')?].to_string())
}

/// The real default microphone (what apps follow), never our own source.
/// Used to pin our capture so making ourselves the default cannot loop.
fn real_default_source(
    routing: &Mutex<Routing>,
    devices: &Mutex<Vec<DeviceInfo>>,
    source_name: &str,
) -> Option<String> {
    let present = |name: &str| {
        devices
            .lock()
            .map(|d| d.iter().any(|x| x.name == name))
            .unwrap_or(false)
    };
    if let Ok(r) = routing.lock() {
        for json in [r.default_current.as_deref(), r.default_prev.as_deref()]
            .into_iter()
            .flatten()
        {
            if let Some(name) = json_name(json)
                && name != source_name
                && !name.starts_with("voice_changer")
                && present(&name)
            {
                return Some(name);
            }
        }
    }
    devices
        .lock()
        .ok()
        .and_then(|d| d.first().map(|x| x.name.clone()))
}

/// Point an application's recording stream at our virtual microphone (or
/// release it), remembering where it was so take-over can be undone.
fn route_stream(
    metadata: Option<&pw::metadata::Metadata>,
    routing: &Mutex<Routing>,
    source_name: &str,
    stream_id: u32,
    on: bool,
) {
    let Some(metadata) = metadata else {
        log::warn!("no default metadata object; cannot route applications");
        return;
    };
    if on {
        let serial = {
            let Ok(mut r) = routing.lock() else { return };
            // Remember the app's current target once, before we move it.
            if !r.original_targets.contains_key(&stream_id) {
                let current = r.targets.get(&stream_id).cloned();
                r.original_targets.insert(stream_id, current);
            }
            r.source_serial.clone()
        };
        match serial {
            Some(serial) => {
                metadata.set_property(stream_id, "target.object", Some("Spa:Id"), Some(&serial));
                if let Ok(mut r) = routing.lock() {
                    r.targets.insert(stream_id, serial);
                }
            }
            None => metadata.set_property(
                stream_id,
                "target.object",
                Some("Spa:String"),
                Some(source_name),
            ),
        }
        log::info!("routing stream {stream_id} to {source_name}");
    } else {
        // Restore the app's original target (or release it to follow default).
        let original = routing.lock().ok().and_then(|mut r| {
            r.targets.remove(&stream_id);
            r.original_targets.remove(&stream_id)
        });
        match original.flatten() {
            Some(orig) => {
                metadata.set_property(stream_id, "target.object", Some("Spa:Id"), Some(&orig))
            }
            None => metadata.set_property(stream_id, "target.object", None, None),
        }
        log::info!("released stream {stream_id}");
    }
}

/// Put every application we moved back on its own microphone.
fn restore_all_apps(metadata: Option<&pw::metadata::Metadata>, routing: &Mutex<Routing>) {
    let Some(metadata) = metadata else { return };
    let apps: Vec<(u32, Option<String>)> = {
        let Ok(mut r) = routing.lock() else { return };
        r.targets.clear();
        r.original_targets.drain().collect()
    };
    for (id, orig) in apps {
        match orig {
            Some(orig) => metadata.set_property(id, "target.object", Some("Spa:Id"), Some(&orig)),
            None => metadata.set_property(id, "target.object", None, None),
        }
    }
}

/// Make (or stop making) our source the session default microphone through
/// `default.configured.audio.source`, remembering what it was before.
fn set_default_source(
    metadata: Option<&pw::metadata::Metadata>,
    routing: &Mutex<Routing>,
    source_name: &str,
    on: bool,
) {
    let Some(metadata) = metadata else {
        if on {
            log::warn!("no default metadata object; cannot set the default microphone");
        }
        return;
    };
    let Ok(mut r) = routing.lock() else { return };
    let ours = format!("{{\"name\":\"{source_name}\"}}");
    if on {
        if !r.default_is_us {
            let current = r.default_current.clone();
            match current {
                Some(c) if !c.contains(source_name) => r.default_prev = Some(c),
                // The default already names us: left behind by a run that
                // did not get to restore it. What it was before that is on disk.
                Some(_) if r.default_prev.is_none() => {
                    r.default_prev = saved_default::load(source_name).flatten();
                }
                _ => {}
            }
            // Remembered on disk too, so a crash or a kill can be undone on
            // the next start.
            saved_default::store(source_name, r.default_prev.as_deref());
            r.default_is_us = true;
        }
        metadata.set_property(
            0,
            "default.configured.audio.source",
            Some("Spa:String:JSON"),
            Some(&ours),
        );
        log::info!("default microphone: {source_name}");
    } else if r.default_is_us {
        r.default_is_us = false;
        match r.default_prev.take() {
            Some(prev) => metadata.set_property(
                0,
                "default.configured.audio.source",
                Some("Spa:String:JSON"),
                Some(&prev),
            ),
            None => metadata.set_property(0, "default.configured.audio.source", None, None),
        }
        saved_default::clear();
        log::info!("default microphone restored");
    }
}

/// A previous run made us the default microphone and never restored it (it
/// crashed or was killed). If this run is not asked to take over, put the
/// remembered default back.
fn recover_default_source(
    metadata: Option<&pw::metadata::Metadata>,
    routing: &Mutex<Routing>,
    source_name: &str,
) {
    let Some(metadata) = metadata else { return };
    let Some(saved) = saved_default::load(source_name) else {
        return;
    };
    let Ok(r) = routing.lock() else { return };
    if r.take_over || r.default_is_us {
        // Taking over again: `set_default_source` picks the saved value up.
        return;
    }
    let stale = r
        .default_current
        .as_deref()
        .and_then(json_name)
        .is_some_and(|n| n == source_name);
    drop(r);
    if stale {
        match saved {
            Some(prev) => metadata.set_property(
                0,
                "default.configured.audio.source",
                Some("Spa:String:JSON"),
                Some(&prev),
            ),
            None => metadata.set_property(0, "default.configured.audio.source", None, None),
        }
        log::info!("restored the default microphone left over from the last run");
    }
    saved_default::clear();
}

/// The default source as it was before we took over, kept in the config
/// folder while we are the default. The file holds our source's name on the
/// first line (so a test instance does not undo the real one) and the
/// metadata value on the second (empty = there was none).
mod saved_default {
    use std::path::PathBuf;

    fn path() -> Option<PathBuf> {
        vc_core::presets::config_dir().map(|d| d.join("default-source.prev"))
    }

    pub fn store(source_name: &str, previous: Option<&str>) {
        let Some(path) = path() else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let text = format!("{source_name}\n{}\n", previous.unwrap_or(""));
        if let Err(e) = std::fs::write(&path, text) {
            log::debug!("could not remember the default microphone: {e}");
        }
    }

    /// `None` = nothing saved for this source; `Some(None)` = saved, and
    /// there was no configured default before.
    pub fn load(source_name: &str) -> Option<Option<String>> {
        let text = std::fs::read_to_string(path()?).ok()?;
        let mut lines = text.lines();
        if lines.next()? != source_name {
            return None;
        }
        Some(
            lines
                .next()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string),
        )
    }

    pub fn clear() {
        if let Some(path) = path() {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn device_from_global(global: &GlobalObject<&spa::utils::dict::DictRef>) -> Option<DeviceInfo> {
    let props = global.props.as_ref()?;
    let class = props.get("media.class")?;
    if class != "Audio/Source" && class != "Audio/Source/Virtual" {
        return None;
    }
    let name = props.get("node.name")?.to_string();
    if name.starts_with("voice_changer") || name.starts_with("vc_") {
        return None;
    }
    let description = props
        .get("node.description")
        .or_else(|| props.get("node.nick"))
        .unwrap_or(&name)
        .to_string();
    Some(DeviceInfo {
        id: global.id,
        serial: props.get("object.serial").map(str::to_string),
        name,
        description,
    })
}

/// Point the capture stream at another source using the session manager's
/// `target.object` metadata (the same mechanism `pactl move-source-output`
/// uses), so the virtual mic keeps running uninterrupted.
fn retarget_capture(
    capture: &pw::stream::Stream,
    metadata: Option<&pw::metadata::Metadata>,
    devices: &Mutex<Vec<DeviceInfo>>,
    target: Option<&str>,
) {
    let Some(metadata) = metadata else {
        log::warn!("no default metadata object; cannot switch microphone at runtime");
        return;
    };
    let node = capture.node_id();
    if node == u32::MAX {
        log::warn!("capture stream has no node id yet; cannot switch microphone");
        return;
    }
    match target {
        None => {
            metadata.set_property(node, "target.object", None, None);
            log::info!("capture target: system default");
        }
        Some(name) => {
            let dev = devices
                .lock()
                .ok()
                .and_then(|list| list.iter().find(|d| d.name == name).cloned());
            match dev {
                Some(DeviceInfo {
                    serial: Some(serial),
                    description,
                    ..
                }) => {
                    metadata.set_property(node, "target.object", Some("Spa:Id"), Some(&serial));
                    log::info!("capture target: {description}");
                }
                Some(dev) => {
                    metadata.set_property(
                        node,
                        "target.object",
                        Some("Spa:String"),
                        Some(&dev.name),
                    );
                    log::info!("capture target: {}", dev.description);
                }
                None => log::warn!("unknown microphone {name:?}; keeping current target"),
            }
        }
    }
}
