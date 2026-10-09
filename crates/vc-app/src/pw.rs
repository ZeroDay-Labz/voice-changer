//! PipeWire I/O: one capture stream from the microphone, one output stream
//! published as a virtual source ("Voice Changer Mic").
//!
//! Both streams live on a dedicated thread that runs the PipeWire main loop.
//! Their realtime `process` callbacks run on PipeWire's data thread and talk
//! to each other through a lock-free SPSC ring buffer. The engine runs inside
//! the *output* callback so the published mic always has fresh data.

use anyhow::{Context as _, Result, anyhow};
use pipewire as pw;
use pw::registry::GlobalObject;
use pw::spa;
use pw::spa::pod::Pod;
use pw::types::ObjectType;
use pw::{properties::properties, stream::StreamFlags};
use spa::param::format::{MediaSubtype, MediaType};
use spa::param::format_utils;
use std::cell::RefCell;
use std::ffi::CStr;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::thread::JoinHandle;
use vc_core::{Meters, Pipeline, VcParams};

pub const SOURCE_NODE_NAME: &str = "voice_changer.source";
pub const SOURCE_DESCRIPTION: &str = "Voice Changer Mic";
pub const CAPTURE_NODE_NAME: &str = "voice_changer.capture";

const SAMPLE_BYTES: usize = std::mem::size_of::<f32>();
/// Largest block we'll ever be asked for in one callback.
const MAX_BLOCK: usize = 8192;

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
}

pub const MONITOR_NODE_NAME: &str = "voice_changer.monitor";

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
}

/// A selectable microphone (any `Audio/Source` node that isn't ours).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: u32,
    pub serial: Option<String>,
    pub name: String,
    pub description: String,
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
}

pub struct AudioThread {
    sender: pw::channel::Sender<Command>,
    thread: Option<JoinHandle<Result<()>>>,
    pub stats: Arc<Stats>,
    pub meters: Arc<Meters>,
    #[cfg(feature = "ai")]
    pub ai_status: Arc<vc_core::ai::AiStatus>,
    devices: Arc<Mutex<Vec<DeviceInfo>>>,
    capture_target: Arc<Mutex<Option<String>>>,
    monitor_on: Arc<AtomicBool>,
    app_streams: Arc<Mutex<Vec<AppStream>>>,
    routing: Arc<Mutex<Routing>>,
    source_name: String,
}

impl AudioThread {
    pub fn spawn(config: AudioConfig, params: Arc<VcParams>) -> Result<Self> {
        ensure_init();
        let (sender, receiver) = pw::channel::channel::<Command>();
        let stats = Arc::new(Stats::default());
        let meters = Arc::new(Meters::default());
        let devices = Arc::new(Mutex::new(Vec::new()));
        let capture_target = Arc::new(Mutex::new(config.capture_target.clone()));
        let monitor_on = Arc::new(AtomicBool::new(false));
        let app_streams = Arc::new(Mutex::new(Vec::new()));
        let routing = Arc::new(Mutex::new(Routing::default()));
        let source_name = config.source_name.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();

        let pipeline = Pipeline::new(config.rate as f32, MAX_BLOCK, params);
        #[cfg(feature = "ai")]
        let ai_status = pipeline.ai_status();
        let ctx = LoopContext {
            config,
            pipeline,
            receiver,
            stats: stats.clone(),
            meters: meters.clone(),
            devices: devices.clone(),
            capture_target: capture_target.clone(),
            monitor_on: monitor_on.clone(),
            app_streams: app_streams.clone(),
            routing: routing.clone(),
            ready: ready_tx,
        };
        let thread = std::thread::Builder::new()
            .name("pipewire-main".into())
            .spawn(move || run_loop(ctx))?;

        // Surface startup errors (no daemon, bad target...) to the caller.
        ready_rx
            .recv()
            .map_err(|_| anyhow!("PipeWire thread died during startup"))??;

        Ok(Self {
            sender,
            thread: Some(thread),
            stats,
            meters,
            #[cfg(feature = "ai")]
            ai_status,
            devices,
            capture_target,
            monitor_on,
            app_streams,
            routing,
            source_name,
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

    /// `node.name` of the microphone we capture from; `None` = system default.
    pub fn capture_target(&self) -> Option<String> {
        self.capture_target.lock().ok().and_then(|t| t.clone())
    }

    pub fn set_capture_target(&self, target: Option<String>) {
        if let Ok(mut t) = self.capture_target.lock() {
            *t = target.clone();
        }
        let _ = self.sender.send(Command::SetCaptureTarget(target));
    }

    pub fn shutdown(mut self) -> Result<()> {
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
    pipeline: Pipeline,
    scratch: Vec<f32>,
    input_wav: Option<Arc<Vec<f32>>>,
    wav_pos: usize,
    stats: Arc<Stats>,
    meters: Arc<Meters>,
}

/// A playback-side callback that drains a ring: used for both the virtual
/// mic and the monitor.
struct SinkData {
    consumer: rtrb::Consumer<f32>,
    /// Keep roughly this many frames queued so a late producer doesn't
    /// starve us; trim anything beyond 2x it. Grows by one quantum on every
    /// underrun (up to `max_slack`), trading a little latency for silence-free output.
    target_slack: usize,
    quantum: usize,
    max_slack: usize,
    /// Output silence until the queue holds a block plus the slack, so the
    /// capture/output callback order within a cycle can't starve us.
    primed: bool,
    stats: Arc<Stats>,
    count_callbacks: bool,
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
    let max_frames = bytes.len() / SAMPLE_BYTES;
    let n = if requested > 0 {
        requested.min(max_frames)
    } else {
        max_frames
    };

    let available = data.consumer.slots();
    let max_queue = n + 2 * data.target_slack;
    if available > max_queue {
        let excess = available - max_queue;
        for _ in 0..excess {
            let _ = data.consumer.pop();
        }
        data.stats
            .trimmed_frames
            .fetch_add(excess as u64, Ordering::Relaxed);
    }
    if !data.primed {
        data.primed = data.consumer.slots() >= n + data.target_slack;
    }
    let have = data.primed && data.consumer.slots() >= n;
    if data.primed && !have {
        data.primed = false;
        if data.target_slack < data.max_slack {
            data.target_slack += data.quantum;
        }
        if data.count_callbacks {
            let n = data.stats.underruns.fetch_add(1, Ordering::Relaxed) + 1;
            if n == 1 || n.is_power_of_two() {
                // Logging from the RT thread is not ideal, but this only
                // happens when audio is already breaking up.
                log::warn!(
                    "output underrun #{n}; buffering grows to {} frames (try --quantum {} if this keeps happening)",
                    data.target_slack,
                    data.quantum * 2
                );
            }
        }
    }
    for dst in bytes.as_chunks_mut::<SAMPLE_BYTES>().0.iter_mut().take(n) {
        let s = if have {
            data.consumer.pop().unwrap_or(0.0)
        } else {
            0.0
        };
        *dst = s.to_le_bytes();
    }
    let chunk = d.chunk_mut();
    *chunk.offset_mut() = 0;
    *chunk.stride_mut() = SAMPLE_BYTES as i32;
    *chunk.size_mut() = (n * SAMPLE_BYTES) as u32;
}

struct LoopContext {
    config: AudioConfig,
    pipeline: Pipeline,
    receiver: pw::channel::Receiver<Command>,
    stats: Arc<Stats>,
    meters: Arc<Meters>,
    devices: Arc<Mutex<Vec<DeviceInfo>>>,
    capture_target: Arc<Mutex<Option<String>>>,
    monitor_on: Arc<AtomicBool>,
    app_streams: Arc<Mutex<Vec<AppStream>>>,
    routing: Arc<Mutex<Routing>>,
    ready: std::sync::mpsc::Sender<Result<()>>,
}

fn run_loop(ctx: LoopContext) -> Result<()> {
    let LoopContext {
        config,
        pipeline,
        receiver,
        stats,
        meters,
        devices,
        capture_target,
        monitor_on,
        app_streams,
        routing,
        ready,
    } = ctx;
    let setup = (|| -> Result<_> {
        let mainloop = pw::main_loop::MainLoopRc::new(None).context("create main loop")?;
        let context = pw::context::ContextRc::new(&mainloop, None).context("create context")?;
        let core = context
            .connect_rc(None)
            .context("connect to PipeWire daemon")?;
        Ok((mainloop, context, core))
    })();
    let (mainloop, _context, core) = match setup {
        Ok(v) => v,
        Err(e) => {
            let _ = ready.send(Err(anyhow!("{e:#}")));
            return Err(e);
        }
    };

    let latency = format!("{}/{}", config.quantum, config.rate);
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
        *pw::keys::AUDIO_CHANNELS => "1",
        "audio.position" => "[MONO]",
    };
    if let Some(target) = &config.capture_target {
        cap_props.insert(*pw::keys::TARGET_OBJECT, target.as_str());
    }
    let capture = pw::stream::StreamRc::new(core.clone(), "Voice Changer capture", cap_props)
        .context("create capture stream")?;
    let _capture_listener = capture
        .add_local_listener_with_user_data(CaptureData {
            to_source,
            to_monitor,
            monitor_on: monitor_on.clone(),
            format: Default::default(),
            pipeline,
            scratch: vec![0.0; MAX_BLOCK],
            input_wav: config.input_wav.clone(),
            wav_pos: 0,
            stats: stats.clone(),
            meters,
        })
        .state_changed(|_, _, old, new| log::info!("capture stream: {old:?} -> {new:?}"))
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
                log::info!(
                    "capture format: {:?} {} Hz x{}",
                    data.format.format(),
                    data.format.rate(),
                    data.format.channels()
                );
            }
        })
        .process(|stream, data| {
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
            let end = (offset + size).min(bytes.len());
            let frame_bytes = SAMPLE_BYTES * channels;
            let frames = ((end - offset) / frame_bytes).min(data.scratch.len());
            let scratch = &mut data.scratch[..frames];
            for (out, frame) in scratch
                .iter_mut()
                .zip(bytes[offset..end].chunks_exact(frame_bytes))
            {
                // Mono fold-down if the capture side ever negotiates >1 channel.
                let mut acc = 0.0f32;
                for s in frame.as_chunks::<SAMPLE_BYTES>().0 {
                    acc += f32::from_le_bytes(*s);
                }
                *out = acc / channels as f32;
            }
            if let Some(wav) = &data.input_wav
                && !wav.is_empty()
            {
                for out in scratch.iter_mut() {
                    *out = wav[data.wav_pos];
                    data.wav_pos = (data.wav_pos + 1) % wav.len();
                }
            }

            let started = std::time::Instant::now();
            data.pipeline.process(scratch);
            let us = started.elapsed().as_micros() as u64;
            data.stats.process_max_us.fetch_max(us, Ordering::Relaxed);
            let avg = data.stats.process_avg_us.load(Ordering::Relaxed);
            data.stats
                .process_avg_us
                .store((avg * 15 + us) / 16, Ordering::Relaxed);
            let (i, o) = data.pipeline.peaks();
            data.meters.publish(i, o, data.pipeline.engine.vad() > 0.5);
            data.meters.publish_auto_gain(data.pipeline.auto_gain_db());

            let monitor = data.monitor_on.load(Ordering::Relaxed);
            for &s in scratch.iter() {
                if data.to_source.push(s).is_err() {
                    data.stats.trimmed_frames.fetch_add(1, Ordering::Relaxed);
                }
                if monitor {
                    let _ = data.to_monitor.push(s);
                }
            }
        })
        .register()
        .context("register capture listener")?;

    // ---- output: published as a virtual microphone --------------------------------------
    let out_props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Playback",
        *pw::keys::MEDIA_ROLE => "Communication",
        *pw::keys::MEDIA_CLASS => "Audio/Source",
        *pw::keys::NODE_NAME => config.source_name.as_str(),
        *pw::keys::NODE_DESCRIPTION => SOURCE_DESCRIPTION,
        *pw::keys::NODE_VIRTUAL => "true",
        *pw::keys::NODE_LATENCY => latency.as_str(),
        *pw::keys::AUDIO_CHANNELS => "1",
        "audio.position" => "[MONO]",
    };
    let output = pw::stream::StreamRc::new(core.clone(), SOURCE_DESCRIPTION, out_props)
        .context("create output stream")?;
    let _output_listener = output
        .add_local_listener_with_user_data(SinkData {
            consumer: from_capture,
            target_slack: (config.quantum as usize).max(64),
            quantum: config.quantum as usize,
            max_slack: (config.quantum as usize) * 4,
            primed: false,
            stats: stats.clone(),
            count_callbacks: true,
        })
        .state_changed(|_, _, old, new| log::info!("source stream: {old:?} -> {new:?}"))
        .process(drain_into)
        .register()
        .context("register output listener")?;

    // ---- monitor: hear yourself through the default output -------------------------------
    let mon_props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Playback",
        *pw::keys::MEDIA_ROLE => "Music",
        *pw::keys::NODE_NAME => MONITOR_NODE_NAME,
        *pw::keys::NODE_DESCRIPTION => "Voice Changer monitor",
        *pw::keys::NODE_LATENCY => latency.as_str(),
        *pw::keys::AUDIO_CHANNELS => "1",
        "audio.position" => "[MONO]",
    };
    let monitor = pw::stream::StreamRc::new(core.clone(), "Voice Changer monitor", mon_props)
        .context("create monitor stream")?;
    let _monitor_listener = monitor
        .add_local_listener_with_user_data(SinkData {
            consumer: from_capture_mon,
            target_slack: (config.quantum as usize).max(64),
            quantum: config.quantum as usize,
            max_slack: (config.quantum as usize) * 4,
            primed: false,
            stats: stats.clone(),
            count_callbacks: false,
        })
        .state_changed(|_, _, old, new| log::info!("monitor stream: {old:?} -> {new:?}"))
        .process(drain_into)
        .register()
        .context("register monitor listener")?;

    let format = mono_f32_format(config.rate);
    let mut cap_params = [Pod::from_bytes(&format).ok_or_else(|| anyhow!("bad format pod"))?];
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
    let mut mon_params = [Pod::from_bytes(&format).ok_or_else(|| anyhow!("bad format pod"))?];
    monitor
        .connect(
            spa::utils::Direction::Output,
            None,
            StreamFlags::AUTOCONNECT
                | StreamFlags::MAP_BUFFERS
                | StreamFlags::RT_PROCESS
                | StreamFlags::INACTIVE,
            &mut mon_params,
        )
        .context("connect monitor stream")?;

    // ---- registry: microphone list + the "default" metadata object -------------------
    let registry = core.get_registry_rc().context("get registry")?;
    let metadata: Rc<RefCell<Option<pw::metadata::Metadata>>> = Rc::new(RefCell::new(None));
    let metadata_listener: Rc<RefCell<Option<pw::metadata::MetadataListener>>> =
        Rc::new(RefCell::new(None));
    let source_node: Rc<RefCell<Option<pw::node::Node>>> = Rc::new(RefCell::new(None));
    let source_name = config.source_name.clone();
    let _registry_listener = registry
        .add_listener_local()
        .global({
            let devices = devices.clone();
            let app_streams = app_streams.clone();
            let routing = routing.clone();
            let metadata = metadata.clone();
            let metadata_listener = metadata_listener.clone();
            let source_node = source_node.clone();
            let registry = registry.downgrade();
            let source_name = source_name.clone();
            move |global| match global.type_ {
                ObjectType::Node => {
                    if let Some(dev) = device_from_global(global)
                        && let Ok(mut list) = devices.lock()
                    {
                        list.retain(|d| d.id != dev.id);
                        list.push(dev);
                        list.sort_by(|a, b| a.description.cmp(&b.description));
                    }
                    let props = global.props.as_ref();
                    if props.and_then(|p| p.get("node.name")) == Some(source_name.as_str()) {
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
                    if let Some(app) = app_stream_from_global(global)
                        && let Ok(mut list) = app_streams.lock()
                    {
                        list.retain(|a| a.id != app.id);
                        list.push(app);
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
                                let routing = routing.clone();
                                let listener = m
                                    .add_listener_local()
                                    .property(move |subject, key, _type, value| {
                                        if let Ok(mut r) = routing.lock() {
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
            move |id| {
                if let Ok(mut list) = devices.lock() {
                    list.retain(|d| d.id != id);
                }
                if let Ok(mut list) = app_streams.lock() {
                    list.retain(|a| a.id != id);
                }
                if let Ok(mut r) = routing.lock() {
                    r.targets.remove(&id);
                }
            }
        })
        .register();

    let _receiver = receiver.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        let devices = devices.clone();
        let metadata = metadata.clone();
        let capture = capture.clone();
        let monitor = monitor.clone();
        let routing = routing.clone();
        let source_name = source_name.clone();
        move |cmd| match cmd {
            Command::Quit => {
                // Give the previous default microphone back before leaving.
                set_default_source(metadata.borrow().as_ref(), &routing, &source_name, false);
                mainloop.quit()
            }
            Command::RouteApp { stream_id, on } => {
                let guard = metadata.borrow();
                let Some(metadata) = guard.as_ref() else {
                    log::warn!("no default metadata object; cannot route applications");
                    return;
                };
                let serial = routing.lock().ok().and_then(|r| r.source_serial.clone());
                if on {
                    match serial {
                        Some(serial) => {
                            metadata.set_property(
                                stream_id,
                                "target.object",
                                Some("Spa:Id"),
                                Some(&serial),
                            );
                            if let Ok(mut r) = routing.lock() {
                                r.targets.insert(stream_id, serial);
                            }
                        }
                        None => metadata.set_property(
                            stream_id,
                            "target.object",
                            Some("Spa:String"),
                            Some(&source_name),
                        ),
                    }
                    log::info!("routing stream {stream_id} to {source_name}");
                } else {
                    metadata.set_property(stream_id, "target.object", None, None);
                    if let Ok(mut r) = routing.lock() {
                        r.targets.remove(&stream_id);
                    }
                    log::info!("released stream {stream_id}");
                }
            }
            Command::SetDefaultSource(on) => {
                set_default_source(metadata.borrow().as_ref(), &routing, &source_name, on)
            }
            Command::SetCaptureTarget(target) => {
                retarget_capture(
                    &capture,
                    metadata.borrow().as_ref(),
                    &devices,
                    target.as_deref(),
                );
            }
            Command::SetMonitor(on) => {
                if let Err(e) = monitor.set_active(on) {
                    log::warn!("monitor set_active({on}) failed: {e}");
                } else {
                    log::info!("monitor {}", if on { "on" } else { "off" });
                }
            }
        }
    });
    log::info!(
        "capture target: {}",
        capture_target
            .lock()
            .ok()
            .and_then(|t| t.clone())
            .unwrap_or_else(|| "system default".into())
    );

    // A saved microphone that no longer exists (unplugged, renamed) must not
    // leave the capture stream pointing at nothing: fall back to the default.
    if let Some(wanted) = capture_target.lock().ok().and_then(|t| t.clone()) {
        let check_at = std::time::Instant::now() + std::time::Duration::from_millis(1500);
        let devices = devices.clone();
        let capture_target = capture_target.clone();
        let capture = capture.clone();
        let metadata = metadata.clone();
        let timer = mainloop.loop_().add_timer(move |_| {
            let known = devices
                .lock()
                .map(|d| d.iter().any(|x| x.name == wanted))
                .unwrap_or(true);
            if !known {
                log::warn!("saved microphone {wanted:?} is not present; using the system default");
                if let Ok(mut t) = capture_target.lock() {
                    *t = None;
                }
                retarget_capture(&capture, metadata.borrow().as_ref(), &devices, None);
            }
        });
        let _ = timer.update_timer(
            Some(check_at.duration_since(std::time::Instant::now())),
            None,
        );
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
            Some(std::time::Duration::from_millis(1500)),
            Some(std::time::Duration::from_secs(5)),
        );
        std::mem::forget(timer);
    }

    let _ = ready.send(Ok(()));
    mainloop.run();
    log::info!("PipeWire loop stopped");
    Ok(())
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

/// Serialize an `EnumFormat` pod for mono 32-bit float at `rate`.
fn mono_f32_format(rate: u32) -> Vec<u8> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(rate);
    info.set_channels(1);
    let mut position = [0u32; spa::param::audio::MAX_CHANNELS];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_MONO;
    info.set_position(position);

    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(spa::pod::Object {
            type_: spa::sys::SPA_TYPE_OBJECT_Format,
            id: spa::sys::SPA_PARAM_EnumFormat,
            properties: info.into(),
        }),
    )
    .expect("serialize audio format")
    .0
    .into_inner()
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
            r.default_prev = current.filter(|c| !c.contains(source_name));
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
        log::info!("default microphone restored");
    }
}

fn device_from_global(global: &GlobalObject<&spa::utils::dict::DictRef>) -> Option<DeviceInfo> {
    let props = global.props.as_ref()?;
    let class = props.get("media.class")?;
    if class != "Audio/Source" && class != "Audio/Source/Virtual" {
        return None;
    }
    let name = props.get("node.name")?.to_string();
    if name.starts_with("voice_changer") || name.starts_with("vc_test") {
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
