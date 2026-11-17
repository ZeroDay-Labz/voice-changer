//! Pick-your-devices audio: one input device, one output device, nothing
//! else. The microphone is processed and the changed voice plays on the
//! chosen output. No virtual device is created and no system setting is
//! touched, so this is what runs on Windows (WASAPI through cpal).
//!
//! A control thread owns both streams. It rebuilds one when its device goes
//! away, when the user picks another, or when the chosen one comes back.
//! The two devices run on their own clocks and often at their own sample
//! rates: each side converts to and from the engine's rate, and the output
//! side absorbs the drift between the clocks (see `sink.rs`, `slip.rs`).

use anyhow::{Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use vc_core::{Meters, Pipeline, VcParams};

use super::block::Processor;
use super::fold::StereoFold;
use super::resample::StreamResampler;
use super::sink::SinkData;
use super::{
    AppStream, AudioConfig, AudioHealth, AudioState, Bench, DeviceInfo, MAX_BLOCK, Stats,
    no_denormals,
};

/// Frames handled per pass inside a callback. Small enough that a block
/// converted up from the lowest supported rate still fits `MAX_BLOCK`.
const CHUNK: usize = 1024;
/// Devices slower than this are refused (the conversion would not fit).
const MIN_RATE: u32 = 8_000;
const RETRY_MIN: Duration = Duration::from_millis(500);
const RETRY_MAX: Duration = Duration::from_secs(5);

/// Test hook: open the devices at this rate instead of their own, to
/// exercise the rate conversion on a machine whose devices all run at the
/// engine's rate (`VC_TEST_DEVICE_RATE=44100`).
fn test_rate() -> Option<u32> {
    std::env::var("VC_TEST_DEVICE_RATE").ok()?.parse().ok()
}

pub fn library_version() -> String {
    format!("cpal ({})", cpal::default_host().id().name())
}

enum Command {
    Quit,
    /// A device choice changed; the shared value is already updated.
    Retarget {
        output: bool,
    },
    /// A stream reported an error. `generation` tells a stream that has
    /// already been replaced from the current one.
    StreamError {
        output: bool,
        generation: u64,
        kind: cpal::ErrorKind,
        message: String,
    },
}

pub struct AudioThread {
    sender: Sender<Command>,
    thread: Option<JoinHandle<()>>,
    pub stats: Arc<Stats>,
    #[cfg(feature = "ai")]
    pub ai_status: Arc<vc_core::ai::AiStatus>,
    inputs: Arc<Mutex<Vec<DeviceInfo>>>,
    outputs: Arc<Mutex<Vec<DeviceInfo>>>,
    capture_target: Arc<Mutex<Option<String>>>,
    active_target: Arc<Mutex<Option<String>>>,
    output_target: Arc<Mutex<Option<String>>>,
    health: Arc<Mutex<AudioHealth>>,
    pub bench: Arc<Bench>,
}

impl AudioThread {
    pub fn spawn(
        config: AudioConfig,
        params: Arc<VcParams>,
        stats: Arc<Stats>,
        meters: Arc<Meters>,
    ) -> Result<Self> {
        let (sender, receiver) = std::sync::mpsc::channel::<Command>();
        let inputs = Arc::new(Mutex::new(Vec::new()));
        let outputs = Arc::new(Mutex::new(Vec::new()));
        let capture_target = Arc::new(Mutex::new(config.capture_target.clone()));
        let active_target = Arc::new(Mutex::new(None));
        let output_target = Arc::new(Mutex::new(config.output_target.clone()));
        let health = Arc::new(Mutex::new(AudioHealth::default()));
        let bench = Arc::new(Bench::new(config.rate));

        let pipeline = Pipeline::new(config.rate as f32, MAX_BLOCK, params);
        #[cfg(feature = "ai")]
        let ai_status = pipeline.ai_status();

        // One second of headroom; the output side trims its queue so the
        // real delay stays around one device period.
        let (to_output, from_input) = rtrb::RingBuffer::<f32>::new(config.rate as usize);
        // The devices hand over audio in their own period (10 ms in WASAPI
        // shared mode), not in ours: the cushion has to cover one of those.
        let mut sink_config = config.clone();
        sink_config.quantum = config.quantum.max(config.rate / 100);
        let input = Arc::new(Mutex::new(InputState {
            channels: 1,
            fold: StereoFold::new(config.rate as f32),
            left: vec![0.0; CHUNK],
            right: vec![0.0; CHUNK],
            mono: vec![0.0; CHUNK],
            resampler: None,
            resampled: vec![0.0; MAX_BLOCK],
            proc: Processor {
                to_source: None,
                to_monitor: to_output,
                // The output device always plays the voice here.
                monitor_on: Arc::new(AtomicBool::new(true)),
                pipeline: Arc::new(Mutex::new(pipeline)),
                input_wav: config.input_wav.clone(),
                wav_pos: 0,
                bench: bench.clone(),
                bench_pos: 0,
                stats: stats.clone(),
                meters,
            },
        }));
        let output = Arc::new(Mutex::new(OutputState {
            channels: 2,
            sink: SinkData::new(from_input, 1, &sink_config, stats.clone(), true, true),
            resampler: None,
            mono: vec![0.0; CHUNK],
        }));

        let ctx = Control {
            rate: config.rate,
            quantum: config.quantum,
            sender: sender.clone(),
            input,
            output,
            inputs: inputs.clone(),
            outputs: outputs.clone(),
            capture_target: capture_target.clone(),
            active_target: active_target.clone(),
            output_target: output_target.clone(),
            health: health.clone(),
            stats: stats.clone(),
        };
        let thread = std::thread::Builder::new()
            .name("audio-control".into())
            .spawn(move || ctx.run(receiver))?;

        Ok(Self {
            sender,
            thread: Some(thread),
            stats,
            #[cfg(feature = "ai")]
            ai_status,
            inputs,
            outputs,
            capture_target,
            active_target,
            output_target,
            health,
            bench,
        })
    }

    // There is no virtual microphone here, so nothing listens to it and
    // nothing can be routed to it.
    pub fn app_streams(&self) -> Vec<AppStream> {
        Vec::new()
    }
    pub fn route_app(&self, _stream_id: u32, _on: bool) {}
    pub fn is_default_source(&self) -> bool {
        false
    }
    pub fn take_over(&self) -> bool {
        false
    }
    pub fn set_default_source(&self, _on: bool) {}
    pub fn listeners(&self) -> usize {
        0
    }

    /// The output device always plays the voice.
    pub fn monitor(&self) -> bool {
        true
    }
    pub fn set_monitor(&self, _on: bool) {}

    pub fn devices(&self) -> Vec<DeviceInfo> {
        self.inputs.lock().map(|d| d.clone()).unwrap_or_default()
    }

    pub fn output_devices(&self) -> Vec<DeviceInfo> {
        self.outputs.lock().map(|d| d.clone()).unwrap_or_default()
    }

    pub fn capture_target(&self) -> Option<String> {
        self.capture_target.lock().ok().and_then(|t| t.clone())
    }

    pub fn active_target(&self) -> Option<String> {
        self.active_target.lock().ok().and_then(|t| t.clone())
    }

    pub fn set_capture_target(&self, target: Option<String>) {
        if let Ok(mut t) = self.capture_target.lock() {
            *t = target;
        }
        let _ = self.sender.send(Command::Retarget { output: false });
    }

    pub fn output_target(&self) -> Option<String> {
        self.output_target.lock().ok().and_then(|t| t.clone())
    }

    pub fn set_output_target(&self, target: Option<String>) {
        if let Ok(mut t) = self.output_target.lock() {
            *t = target;
        }
        let _ = self.sender.send(Command::Retarget { output: true });
    }

    pub fn health(&self) -> AudioHealth {
        self.health.lock().map(|h| h.clone()).unwrap_or_default()
    }

    pub fn shutdown(mut self) -> Result<()> {
        let _ = self.sender.send(Command::Quit);
        if let Some(t) = self.thread.take() {
            t.join().map_err(|_| anyhow!("audio thread panicked"))?;
        }
        Ok(())
    }
}

// ----------------------------------------------------------------- realtime

/// The microphone side: whatever the device delivers becomes mono at the
/// engine's rate and goes through the pipeline.
struct InputState {
    channels: usize,
    fold: StereoFold,
    left: Vec<f32>,
    right: Vec<f32>,
    mono: Vec<f32>,
    resampler: Option<StreamResampler>,
    resampled: Vec<f32>,
    proc: Processor,
}

impl InputState {
    fn feed<T>(&mut self, data: &[T])
    where
        T: cpal::SizedSample,
        f32: cpal::FromSample<T>,
    {
        use cpal::FromSample;
        no_denormals();
        self.proc
            .stats
            .capture_callbacks
            .fetch_add(1, Ordering::Relaxed);
        let channels = self.channels.max(1);
        for chunk in data.chunks(channels * CHUNK) {
            let frames = chunk.len() / channels;
            let mono = &mut self.mono[..frames];
            if channels == 1 {
                for (m, s) in mono.iter_mut().zip(chunk) {
                    *m = f32::from_sample_(*s);
                }
            } else {
                // The first two inputs: a stereo microphone, or an interface
                // with the microphone on input 1 or 2 (the fold finds it).
                let (left, right) = (&mut self.left[..frames], &mut self.right[..frames]);
                for ((l, r), frame) in left
                    .iter_mut()
                    .zip(right.iter_mut())
                    .zip(chunk.chunks_exact(channels))
                {
                    *l = f32::from_sample_(frame[0]);
                    *r = f32::from_sample_(frame[1]);
                }
                self.fold.process(left, right, mono);
            }
            match self.resampler.as_mut() {
                None => self.proc.process(mono),
                Some(r) => {
                    let n = r.push(mono, &mut self.resampled);
                    self.proc.process(&mut self.resampled[..n]);
                }
            }
        }
    }
}

/// The output side: pulls the voice from the queue and hands it to the
/// device in its rate, format and channel count.
struct OutputState {
    channels: usize,
    sink: SinkData,
    resampler: Option<StreamResampler>,
    mono: Vec<f32>,
}

impl OutputState {
    fn fill<T>(&mut self, data: &mut [T])
    where
        T: cpal::SizedSample + cpal::FromSample<f32>,
    {
        no_denormals();
        let channels = self.channels.max(1);
        for chunk in data.chunks_mut(channels * CHUNK) {
            let frames = chunk.len() / channels;
            let voice: &[f32] = match self.resampler.as_mut() {
                None => {
                    self.sink.render(frames);
                    &self.sink.block[..frames]
                }
                Some(r) => {
                    let need = r.input_needed(frames).min(MAX_BLOCK);
                    self.sink.render(need);
                    r.pull(&self.sink.block[..need], &mut self.mono[..frames]);
                    &self.mono[..frames]
                }
            };
            for (frame, &s) in chunk.chunks_exact_mut(channels).zip(voice) {
                let v = T::from_sample_(s.clamp(-1.0, 1.0));
                // Front left and right. The rest of a surround layout
                // (centre, subwoofer, rears) stays silent.
                for (i, dst) in frame.iter_mut().enumerate() {
                    *dst = if i < 2 { v } else { T::EQUILIBRIUM };
                }
            }
        }
    }
}

// ------------------------------------------------------------------ control

struct Control {
    rate: u32,
    quantum: u32,
    sender: Sender<Command>,
    input: Arc<Mutex<InputState>>,
    output: Arc<Mutex<OutputState>>,
    inputs: Arc<Mutex<Vec<DeviceInfo>>>,
    outputs: Arc<Mutex<Vec<DeviceInfo>>>,
    capture_target: Arc<Mutex<Option<String>>>,
    active_target: Arc<Mutex<Option<String>>>,
    output_target: Arc<Mutex<Option<String>>>,
    health: Arc<Mutex<AudioHealth>>,
    stats: Arc<Stats>,
}

/// One direction's stream and what is known about it.
struct Side {
    stream: Option<cpal::Stream>,
    generation: u64,
    /// The device id in use (`None` = the system default).
    active: Option<String>,
    /// Why there is no stream, in plain words.
    problem: Option<String>,
    retry_at: Instant,
    backoff: Duration,
}

impl Side {
    fn new() -> Self {
        Self {
            stream: None,
            generation: 0,
            active: None,
            problem: None,
            retry_at: Instant::now(),
            backoff: RETRY_MIN,
        }
    }

    /// Close the stream and open it again at the next opportunity.
    fn restart(&mut self) {
        self.stream = None;
        self.retry_at = Instant::now();
        self.backoff = RETRY_MIN;
    }

    fn failed(&mut self, why: String) {
        self.stream = None;
        self.problem = Some(why);
        self.retry_at = Instant::now() + self.backoff;
        self.backoff = (self.backoff * 2).min(RETRY_MAX);
    }
}

fn set_health(health: &Mutex<AudioHealth>, f: impl FnOnce(&mut AudioHealth)) {
    if let Ok(mut h) = health.lock() {
        f(&mut h);
    }
}

impl Control {
    fn run(self, receiver: Receiver<Command>) {
        let host = cpal::default_host();
        log::info!("audio backend: {}", host.id().name());
        let mut input = Side::new();
        let mut output = Side::new();
        let mut last_scan = Instant::now() - Duration::from_secs(10);
        let mut last_alive = Instant::now();
        let mut last_callbacks = 0u64;
        let mut quiet_seconds = 0u32;
        let mut mic_alive = true;

        loop {
            let now = Instant::now();
            // The device lists, and with them: has the chosen device come
            // back, has the one in use gone?
            if now.duration_since(last_scan) >= Duration::from_secs(2) {
                last_scan = now;
                self.scan(&host);
                self.follow_choice(&mut input, &self.capture_target, &self.inputs);
                self.follow_choice(&mut output, &self.output_target, &self.outputs);
            }
            // Output first, so the voice has somewhere to go when the
            // microphone starts.
            if output.stream.is_none() && now >= output.retry_at {
                self.open(&host, &mut output, true);
            }
            if input.stream.is_none() && now >= input.retry_at {
                self.open(&host, &mut input, false);
                if let Ok(mut a) = self.active_target.lock() {
                    *a = input.active.clone();
                }
            }
            if now.duration_since(last_alive) >= Duration::from_secs(1) {
                last_alive = now;
                let callbacks = self.stats.capture_callbacks.load(Ordering::Relaxed);
                if callbacks != last_callbacks {
                    quiet_seconds = 0;
                } else {
                    quiet_seconds = quiet_seconds.saturating_add(1);
                }
                last_callbacks = callbacks;
                // Two quiet seconds in a row: nothing is arriving.
                mic_alive = quiet_seconds < 2;
            }
            let running = input.stream.is_some() && output.stream.is_some();
            let message = [&input.problem, &output.problem]
                .into_iter()
                .flatten()
                .cloned()
                .collect::<Vec<_>>()
                .join(" ");
            set_health(&self.health, |h| {
                h.state = if running {
                    AudioState::Running
                } else if h.sessions == 0 {
                    AudioState::Starting
                } else {
                    AudioState::Reconnecting
                };
                h.message = message;
                h.mic_alive = mic_alive && input.stream.is_some();
                h.virtual_mic = false;
            });

            match receiver.recv_timeout(Duration::from_millis(200)) {
                Ok(Command::Quit) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(Command::Retarget { output: false }) => input.restart(),
                Ok(Command::Retarget { output: true }) => output.restart(),
                Ok(Command::StreamError {
                    output: is_output,
                    generation,
                    kind,
                    message,
                }) => {
                    let side = if is_output { &mut output } else { &mut input };
                    if generation != side.generation {
                        continue;
                    }
                    // The system moved a default-following stream to the new
                    // default device by itself: nothing to rebuild.
                    if kind == cpal::ErrorKind::DeviceChanged {
                        log::info!("audio device changed: {message}");
                        continue;
                    }
                    let what = if is_output { "output" } else { "microphone" };
                    log::warn!("{what} stream stopped ({kind:?}: {message}); reopening");
                    side.failed(format!(
                        "The {what} stopped working ({message}). Trying again…"
                    ));
                    last_scan = Instant::now() - Duration::from_secs(10);
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
        // Streams close here, on the thread that made them.
        drop(input);
        drop(output);
        log::info!("audio stopped");
    }

    /// Refresh both device lists.
    fn scan(&self, host: &cpal::Host) {
        let list = |devices: Vec<cpal::Device>, output: bool| {
            let mut list: Vec<DeviceInfo> = devices
                .into_iter()
                .filter_map(|d| {
                    let id = d.id().ok()?.to_string();
                    let description = d
                        .description()
                        .map(|x| x.name().to_string())
                        .unwrap_or_else(|_| id.clone());
                    Some(DeviceInfo {
                        output,
                        id: 0,
                        serial: None,
                        name: id,
                        description,
                    })
                })
                .collect();
            list.sort_by(|a, b| a.description.cmp(&b.description));
            list.dedup_by(|a, b| a.name == b.name);
            list
        };
        if let Ok(devices) = host.input_devices()
            && let Ok(mut l) = self.inputs.lock()
        {
            *l = list(devices.collect(), false);
        }
        if let Ok(devices) = host.output_devices()
            && let Ok(mut l) = self.outputs.lock()
        {
            *l = list(devices.collect(), true);
        }
    }

    /// Reopen a side when the user's device has come back, or when the one
    /// in use is no longer there.
    fn follow_choice(
        &self,
        side: &mut Side,
        target: &Mutex<Option<String>>,
        present: &Mutex<Vec<DeviceInfo>>,
    ) {
        if side.stream.is_none() {
            return;
        }
        let wanted = target.lock().ok().and_then(|t| t.clone());
        let is_present = |id: &str| {
            present
                .lock()
                .map(|l| l.iter().any(|d| d.name == id))
                .unwrap_or(false)
        };
        let back = wanted.as_deref().is_some_and(is_present) && side.active != wanted;
        let gone = side.active.as_deref().is_some_and(|id| !is_present(id));
        if back || gone {
            side.restart();
        }
    }

    /// Open one side's stream on the chosen device, or the default when
    /// that one is not there.
    fn open(&self, host: &cpal::Host, side: &mut Side, is_output: bool) {
        let what = if is_output { "output" } else { "microphone" };
        let target = if is_output {
            &self.output_target
        } else {
            &self.capture_target
        };
        let wanted = target.lock().ok().and_then(|t| t.clone());
        let chosen = wanted.as_ref().and_then(|id| {
            let id = cpal::DeviceId::from_str(id).ok()?;
            host.device_by_id(&id)
        });
        let (device, active) = match chosen {
            Some(d) => (Some(d), wanted.clone()),
            None => {
                if let Some(id) = &wanted {
                    log::warn!("{what} {id:?} is not present; using the system default");
                }
                let default = if is_output {
                    host.default_output_device()
                } else {
                    host.default_input_device()
                };
                (default, None)
            }
        };
        let Some(device) = device else {
            side.failed(if is_output {
                "No output device found. Connect headphones or speakers.".into()
            } else {
                "No microphone found. Plug one in.".into()
            });
            return;
        };
        side.generation += 1;
        let built = if is_output {
            self.build_output(&device, side.generation)
        } else {
            self.build_input(&device, side.generation)
        };
        match built.and_then(|s| s.play().map(|()| s).map_err(|e| anyhow!("{e}"))) {
            Ok(stream) => {
                let name = device
                    .description()
                    .map(|d| d.name().to_string())
                    .unwrap_or_default();
                log::info!("{what}: {name}");
                side.stream = Some(stream);
                side.active = active;
                side.problem = None;
                side.backoff = RETRY_MIN;
                if !is_output {
                    set_health(&self.health, |h| h.sessions += 1);
                }
            }
            Err(e) => {
                log::warn!("could not open the {what}: {e:#}");
                side.failed(format!("The {what} could not be opened ({e:#})."));
            }
        }
    }

    /// The stream settings to try: a short buffer when the device says it
    /// can do one, then whatever the device prefers.
    fn configs(&self, supported: &cpal::SupportedStreamConfig) -> Vec<cpal::StreamConfig> {
        let base = supported.config();
        let mut list = Vec::new();
        let wanted =
            (self.quantum as u64 * supported.sample_rate() as u64 / self.rate as u64) as u32;
        if let cpal::SupportedBufferSize::Range { min, max } = supported.buffer_size()
            && (*min..=*max).contains(&wanted)
        {
            let mut short = base;
            short.buffer_size = cpal::BufferSize::Fixed(wanted);
            list.push(short);
        }
        let mut default = base;
        default.buffer_size = cpal::BufferSize::Default;
        list.push(default);
        list
    }

    fn on_error(&self, output: bool, generation: u64) -> impl FnMut(cpal::Error) + Send + 'static {
        let sender = self.sender.clone();
        move |e| {
            let _ = sender.send(Command::StreamError {
                output,
                generation,
                kind: e.kind(),
                message: e.to_string(),
            });
        }
    }

    fn build_input(&self, device: &cpal::Device, generation: u64) -> Result<cpal::Stream> {
        let supported = match test_rate() {
            Some(rate) => device
                .supported_input_configs()
                .map_err(|e| anyhow!("{e}"))?
                .find_map(|c| c.try_with_sample_rate(rate))
                .ok_or_else(|| anyhow!("no {rate} Hz mode"))?,
            None => device.default_input_config().map_err(|e| anyhow!("{e}"))?,
        };
        let rate = supported.sample_rate();
        if rate < MIN_RATE {
            return Err(anyhow!("its sample rate of {rate} Hz is too low"));
        }
        {
            let mut state = self
                .input
                .lock()
                .map_err(|_| anyhow!("audio state is poisoned"))?;
            state.channels = supported.channels() as usize;
            state.fold.reset();
            state.resampler =
                (rate != self.rate).then(|| StreamResampler::new(rate, self.rate, CHUNK));
        }
        log::info!(
            "microphone format: {:?} {} Hz x{}",
            supported.sample_format(),
            rate,
            supported.channels()
        );
        let mut last = None;
        for config in self.configs(&supported) {
            macro_rules! build {
                ($t:ty) => {{
                    let state = self.input.clone();
                    device.build_input_stream(
                        config,
                        move |data: &[$t], _: &cpal::InputCallbackInfo| {
                            if let Ok(mut s) = state.try_lock() {
                                s.feed(data);
                            }
                        },
                        self.on_error(false, generation),
                        None,
                    )
                }};
            }
            let built = match supported.sample_format() {
                cpal::SampleFormat::F32 => build!(f32),
                cpal::SampleFormat::I16 => build!(i16),
                cpal::SampleFormat::U16 => build!(u16),
                cpal::SampleFormat::I32 => build!(i32),
                cpal::SampleFormat::I24 => build!(cpal::I24),
                cpal::SampleFormat::I8 => build!(i8),
                cpal::SampleFormat::U8 => build!(u8),
                cpal::SampleFormat::F64 => build!(f64),
                other => return Err(anyhow!("its sample format {other:?} is not supported")),
            };
            match built {
                Ok(s) => return Ok(s),
                Err(e) => last = Some(e),
            }
        }
        Err(anyhow!(
            "{}",
            last.map(|e| e.to_string()).unwrap_or_default()
        ))
    }

    fn build_output(&self, device: &cpal::Device, generation: u64) -> Result<cpal::Stream> {
        let supported = match test_rate() {
            Some(rate) => device
                .supported_output_configs()
                .map_err(|e| anyhow!("{e}"))?
                .find_map(|c| c.try_with_sample_rate(rate))
                .ok_or_else(|| anyhow!("no {rate} Hz mode"))?,
            None => device.default_output_config().map_err(|e| anyhow!("{e}"))?,
        };
        let rate = supported.sample_rate();
        if rate < MIN_RATE {
            return Err(anyhow!("its sample rate of {rate} Hz is too low"));
        }
        {
            let mut state = self
                .output
                .lock()
                .map_err(|_| anyhow!("audio state is poisoned"))?;
            state.channels = supported.channels() as usize;
            state.resampler =
                (rate != self.rate).then(|| StreamResampler::new(self.rate, rate, MAX_BLOCK));
            // What is queued was meant for the previous device.
            state.sink.flush.store(true, Ordering::Relaxed);
        }
        log::info!(
            "output format: {:?} {} Hz x{}",
            supported.sample_format(),
            rate,
            supported.channels()
        );
        let mut last = None;
        for config in self.configs(&supported) {
            macro_rules! build {
                ($t:ty) => {{
                    let state = self.output.clone();
                    device.build_output_stream(
                        config,
                        move |data: &mut [$t], _: &cpal::OutputCallbackInfo| {
                            if let Ok(mut s) = state.try_lock() {
                                s.fill(data);
                            }
                        },
                        self.on_error(true, generation),
                        None,
                    )
                }};
            }
            let built = match supported.sample_format() {
                cpal::SampleFormat::F32 => build!(f32),
                cpal::SampleFormat::I16 => build!(i16),
                cpal::SampleFormat::U16 => build!(u16),
                cpal::SampleFormat::I32 => build!(i32),
                cpal::SampleFormat::I24 => build!(cpal::I24),
                cpal::SampleFormat::I8 => build!(i8),
                cpal::SampleFormat::U8 => build!(u8),
                cpal::SampleFormat::F64 => build!(f64),
                other => return Err(anyhow!("its sample format {other:?} is not supported")),
            };
            match built {
                Ok(s) => return Ok(s),
                Err(e) => last = Some(e),
            }
        }
        Err(anyhow!(
            "{}",
            last.map(|e| e.to_string()).unwrap_or_default()
        ))
    }
}
