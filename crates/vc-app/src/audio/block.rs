//! What happens to one block of microphone audio, whatever delivered it:
//! test input and bench clips are swapped in, the pipeline runs, the meters
//! are published and the result is queued for the outputs.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use vc_core::{Meters, Pipeline};

use super::{Bench, Stats, push_all};

pub(crate) struct Processor {
    /// The stream other applications record from (PipeWire's virtual
    /// microphone). `None` where there is no such thing.
    pub to_source: Option<rtrb::Producer<f32>>,
    /// The stream the user listens to ("Hear myself", or the one output of
    /// the pick-your-devices mode).
    pub to_monitor: rtrb::Producer<f32>,
    pub monitor_on: Arc<AtomicBool>,
    /// Shared with the backend thread so it survives a reconnect. Only the
    /// capture callback locks it while a session runs; `try_lock` keeps the
    /// RT path free of blocking.
    pub pipeline: Arc<Mutex<Pipeline>>,
    pub input_wav: Option<Arc<Vec<f32>>>,
    pub wav_pos: usize,
    pub bench: Arc<Bench>,
    pub bench_pos: usize,
    pub stats: Arc<Stats>,
    pub meters: Arc<Meters>,
}

impl Processor {
    /// Realtime: process one mono block in place and queue it.
    pub fn process(&mut self, scratch: &mut [f32]) {
        if let Some(wav) = &self.input_wav
            && !wav.is_empty()
        {
            for out in scratch.iter_mut() {
                *out = wav[self.wav_pos];
                self.wav_pos = (self.wav_pos + 1) % wav.len();
            }
        }

        // Test bench: keep the raw microphone if a recording runs, and
        // replay a clip in its place if one is playing.
        self.bench.capture(scratch);
        let bench_clip = self.bench.play.load();
        let benching = bench_clip.is_some();
        if let Some(clip) = bench_clip.as_ref() {
            for out in scratch.iter_mut() {
                self.bench_pos %= clip.len();
                *out = clip[self.bench_pos];
                self.bench_pos += 1;
            }
        }
        drop(bench_clip);

        // Uncontended except for the instant a session is torn down.
        let Ok(mut pipeline) = self.pipeline.try_lock() else {
            self.stats.skipped_callbacks.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let started = std::time::Instant::now();
        pipeline.process(scratch);
        let us = started.elapsed().as_micros() as u64;
        self.stats.process_max_us.fetch_max(us, Ordering::Relaxed);
        let avg = self.stats.process_avg_us.load(Ordering::Relaxed);
        self.stats
            .process_avg_us
            .store((avg * 15 + us) / 16, Ordering::Relaxed);
        let (i, o) = pipeline.peaks();
        self.meters.publish(i, o, pipeline.engine.vad() > 0.5);
        self.meters.publish_auto_gain(pipeline.auto_gain_db());
        drop(pipeline);

        if self.monitor_on.load(Ordering::Relaxed) {
            push_all(&mut self.to_monitor, scratch, None);
        }
        if let Some(to_source) = self.to_source.as_mut() {
            // A bench clip is for the user's ears only: the virtual
            // microphone carries silence while one plays.
            if benching {
                scratch.fill(0.0);
            }
            push_all(to_source, scratch, Some(&self.stats));
        }
    }
}
