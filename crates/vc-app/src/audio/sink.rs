//! The playback side of a ring: a realtime callback asks for a block and
//! gets the queued voice, or silence while the queue refills. Shared by
//! every stream that plays the voice, whatever the backend.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::slip::Slip;
use super::{AudioConfig, MAX_BLOCK, Stats};

/// A playback-side callback that drains a ring: used for both the virtual
/// mic and the monitor.
pub(crate) struct SinkData {
    pub(crate) consumer: rtrb::Consumer<f32>,
    /// Interleaved channels of the stream; the mono voice goes to all of them.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) channels: usize,
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
    /// Frames left in which an underrun means the last shrink went too far.
    probation: usize,
    /// Output silence until the queue holds a block plus the slack, so the
    /// capture/output callback order within a cycle can't starve us.
    primed: bool,
    /// Frames of fade-in still to apply after a gap or a trim, so the voice
    /// comes back without a click.
    fade_left: usize,
    /// Set from the main thread to throw away whatever is queued (the monitor
    /// was just switched on: what the ring holds is stale).
    pub(crate) flush: Arc<AtomicBool>,
    stats: Arc<Stats>,
    count_callbacks: bool,
    /// Set for a stream that plays on another device's clock (the monitor):
    /// the fill level is steered by stretching blocks by one sample.
    drift: Option<Slip>,
    /// Samples popped from the ring for this block (one more than a block
    /// when catching up), and the block as it will be played.
    staging: Vec<f32>,
    pub(crate) block: Vec<f32>,
}

/// Length of the fade-in after a discontinuity (about 2.7 ms at 48 kHz).
pub(crate) const FADE_FRAMES: usize = 128;

impl SinkData {
    pub(crate) fn new(
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
            probation: 0,
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

impl SinkData {
    /// Realtime: put the next `n` frames (at most `MAX_BLOCK`) into
    /// `self.block`, silence where there is nothing to play. Keeps the
    /// queue short, grows the cushion after an underrun and fades in after
    /// every gap.
    pub(crate) fn render(&mut self, n: usize) {
        let n = n.min(MAX_BLOCK);
        if self.count_callbacks {
            self.stats.output_callbacks.fetch_add(1, Ordering::Relaxed);
        }
        if self.flush.swap(false, Ordering::Relaxed) {
            self.discard(usize::MAX);
            self.primed = false;
        }
        let available = self.consumer.slots();
        let max_queue = n + 2 * self.target_slack;
        if available > max_queue {
            // Too much queued (nobody was reading, or the slack just shrank):
            // skip ahead to keep the delay bounded, and fade back in over the cut.
            let excess = available - max_queue;
            self.discard(excess);
            self.fade_left = FADE_FRAMES;
            self.stats
                .trimmed_frames
                .fetch_add(excess as u64, Ordering::Relaxed);
        }
        if !self.primed {
            self.primed = self.consumer.slots() >= n + self.target_slack;
            if self.primed {
                self.fade_left = FADE_FRAMES;
            }
        }
        let have = self.primed && self.consumer.slots() >= n;
        if self.primed && !have {
            self.primed = false;
            self.calm_frames = 0;
            if self.target_slack < self.max_slack {
                self.target_slack += self.quantum;
            }
            // Taking slack back just caused this: that much is needed here,
            // so do not try again (a dropout every half minute otherwise).
            if self.probation > 0 {
                self.probation = 0;
                self.min_slack = self.target_slack;
            }
            if self.count_callbacks {
                // Counted only: the window shows dropouts, and logging (which
                // allocates and takes a lock) does not belong on the RT thread.
                self.stats.underruns.fetch_add(1, Ordering::Relaxed);
            }
        }
        if !have || n == 0 {
            self.block[..n].fill(0.0);
            return;
        }
        self.calm_frames += n;
        self.probation = self.probation.saturating_sub(n);
        if self.calm_frames >= self.relax_after {
            self.calm_frames = 0;
            if self.target_slack > self.min_slack {
                self.target_slack -= self.quantum.min(self.target_slack - self.min_slack);
                self.probation = self.relax_after * 2;
            }
        }
        // Fetch the block: straight from the ring, or one sample more or fewer
        // stretched to fit when this stream follows another clock.
        let available = self.consumer.slots();
        let take = match &self.drift {
            Some(_) => Slip::take(available, n, n + self.target_slack, self.quantum / 2),
            None => n,
        }
        .min(available);
        for staged in self.staging[..take].iter_mut() {
            *staged = self.consumer.pop().unwrap_or(0.0);
        }
        match self.drift.as_mut() {
            Some(slip) => slip.stretch(&self.staging[..take], &mut self.block[..n]),
            None => self.block[..n].copy_from_slice(&self.staging[..n]),
        }
        for s in &mut self.block[..n] {
            if self.fade_left == 0 {
                break;
            }
            *s *= 1.0 - self.fade_left as f32 / FADE_FRAMES as f32;
            self.fade_left -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> AudioConfig {
        AudioConfig {
            rate: 48_000,
            quantum: 256,
            capture_target: None,
            source_name: String::new(),
            input_wav: None,
            output_target: None,
        }
    }

    /// A producer that delivers in bursts of `burst` frames while the reader
    /// takes `n` at a time: the queue needs about a burst of cushion.
    fn run(sink: &mut SinkData, tx: &mut rtrb::Producer<f32>, seconds: usize, burst: usize) {
        let n = 256;
        let mut owed = 0usize;
        for _ in 0..seconds * 48_000 / n {
            owed += n;
            if owed >= burst {
                for _ in 0..burst {
                    let _ = tx.push(0.25);
                }
                owed -= burst;
            }
            sink.render(n);
        }
    }

    #[test]
    fn a_shrink_that_causes_a_dropout_is_not_repeated() {
        let stats = Arc::new(Stats::default());
        let (mut tx, rx) = rtrb::RingBuffer::<f32>::new(48_000);
        let mut sink = SinkData::new(rx, 1, &config(), stats.clone(), true, false);
        // Bursts of four blocks: the cushion has to grow.
        run(&mut sink, &mut tx, 20, 1024);
        let learned = stats.underruns.load(Ordering::Relaxed);
        assert!(learned >= 1, "the bursts should have caused a dropout");
        // Each level of cushion is given up once and taken back for good
        // when that drops out; after that it stays quiet.
        run(&mut sink, &mut tx, 600, 1024);
        let settling = stats.underruns.load(Ordering::Relaxed) - learned;
        assert!(settling <= 3, "{settling} dropouts while settling");
        let settled = stats.underruns.load(Ordering::Relaxed);
        run(&mut sink, &mut tx, 600, 1024);
        let later = stats.underruns.load(Ordering::Relaxed) - settled;
        assert_eq!(later, 0, "dropouts after settling");
    }

    #[test]
    fn silence_until_primed_then_the_voice() {
        let stats = Arc::new(Stats::default());
        let (mut tx, rx) = rtrb::RingBuffer::<f32>::new(48_000);
        let mut sink = SinkData::new(rx, 1, &config(), stats, true, false);
        sink.render(256);
        assert!(sink.block[..256].iter().all(|s| *s == 0.0));
        for _ in 0..512 {
            let _ = tx.push(0.5);
        }
        sink.render(256);
        // Faded in over the first samples, full level after.
        assert!(sink.block[0].abs() < 0.01);
        assert!((sink.block[255] - 0.5).abs() < 1e-6);
    }
}
