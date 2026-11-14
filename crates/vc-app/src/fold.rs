//! Stereo → mono fold-down for the microphone input.
//!
//! A plain average is right for a mono microphone (both channels carry the
//! same signal) and for a real stereo microphone. It is wrong in two common
//! set-ups:
//!
//! * an audio interface or motherboard jack with the microphone on one
//!   channel and nothing on the other: the average halves the voice;
//! * a balanced cable into a stereo input, where the two channels carry the
//!   same signal with opposite polarity: the average cancels it.
//!
//! `StereoFold` watches both channels and uses only the live one in the first
//! case and the difference in the second. Decisions are made once per block
//! from slow level estimates and applied through smoothed weights, so the
//! switch is inaudible. Realtime-safe: no allocation, no locks.

/// A channel has to have been at least this loud (mean power) before the
/// other one can be judged dead: about -50 dBFS.
const LIVE_FLOOR: f32 = 1e-5;
/// "Dead" = at least 30 dB below the live channel's recent level...
const DEAD_RATIO: f32 = 1e-3;
/// ...and alive again once it comes within 20 dB of it.
const REVIVE_RATIO: f32 = 1e-2;
/// The level estimates lose half their power every this many seconds.
const HOLD_HALF_LIFE_SECS: f32 = 10.0;
/// Weight smoothing time constant.
const SMOOTH_SECS: f32 = 0.010;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldMode {
    /// Average of both channels.
    Both,
    /// Only this channel (0 = left, 1 = right) carries the microphone.
    Only(usize),
    /// The channels are the same signal with opposite polarity.
    Inverted,
}

pub struct StereoFold {
    sample_rate: f32,
    /// Recent peak of each channel's block power, slowly decaying.
    hold: [f32; 2],
    mode: FoldMode,
    /// Current (smoothed) weights applied to left and right.
    weights: [f32; 2],
    smooth: f32,
}

impl StereoFold {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            hold: [0.0; 2],
            mode: FoldMode::Both,
            weights: [0.5, 0.5],
            smooth: 1.0 - (-1.0 / (SMOOTH_SECS * sample_rate)).exp(),
        }
    }

    /// Forget what was learned (the microphone changed).
    pub fn reset(&mut self) {
        self.hold = [0.0; 2];
        self.mode = FoldMode::Both;
        self.weights = [0.5, 0.5];
    }

    #[cfg(test)]
    pub fn mode(&self) -> FoldMode {
        self.mode
    }

    /// Fold `left`/`right` into `out` (all the same length).
    pub fn process(&mut self, left: &[f32], right: &[f32], out: &mut [f32]) {
        let n = out.len().min(left.len()).min(right.len());
        if n == 0 {
            return;
        }
        let (mut pl, mut pr, mut psum) = (0.0f32, 0.0f32, 0.0f32);
        for (&l, &r) in left[..n].iter().zip(&right[..n]) {
            pl += l * l;
            pr += r * r;
            let s = 0.5 * (l + r);
            psum += s * s;
        }
        let inv = 1.0 / n as f32;
        let power = [pl * inv, pr * inv];
        let psum = psum * inv;

        let decay = 0.5f32.powf(n as f32 / (self.sample_rate * HOLD_HALF_LIFE_SECS));
        for (hold, &p) in self.hold.iter_mut().zip(&power) {
            *hold = (*hold * decay).max(p);
        }
        self.mode = self.decide(power, psum);

        let target = match self.mode {
            FoldMode::Both => [0.5, 0.5],
            FoldMode::Only(0) => [1.0, 0.0],
            FoldMode::Only(_) => [0.0, 1.0],
            FoldMode::Inverted => [0.5, -0.5],
        };
        let a = self.smooth;
        let [mut wl, mut wr] = self.weights;
        for ((o, &l), &r) in out[..n].iter_mut().zip(&left[..n]).zip(&right[..n]) {
            wl += a * (target[0] - wl);
            wr += a * (target[1] - wr);
            *o = wl * l + wr * r;
        }
        self.weights = [wl, wr];
    }

    fn decide(&self, power: [f32; 2], psum: f32) -> FoldMode {
        let mean = 0.5 * (power[0] + power[1]);
        // Opposite polarity: both channels are loud and alike, yet their
        // average is almost nothing. Checked on the current block only, so a
        // quiet moment keeps whatever was decided before.
        let alike = power[0].min(power[1]) > 0.25 * power[0].max(power[1]);
        if mean > LIVE_FLOOR && alike {
            if psum < 0.01 * mean {
                return FoldMode::Inverted;
            }
            if self.mode == FoldMode::Inverted && psum > 0.1 * mean {
                return FoldMode::Both;
            }
        }
        match self.mode {
            FoldMode::Inverted => FoldMode::Inverted,
            FoldMode::Only(live) => {
                let dead = 1 - live;
                // The silent channel woke up (a second microphone, a mono
                // source that is now on both channels).
                if power[dead] > self.hold[live] * REVIVE_RATIO {
                    FoldMode::Both
                } else {
                    FoldMode::Only(live)
                }
            }
            FoldMode::Both => {
                for live in 0..2 {
                    let dead = 1 - live;
                    if self.hold[live] > LIVE_FLOOR
                        && self.hold[dead] < self.hold[live] * DEAD_RATIO
                    {
                        return FoldMode::Only(live);
                    }
                }
                FoldMode::Both
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;
    const BLOCK: usize = 256;

    fn tone(amp: f32, blocks: usize) -> Vec<f32> {
        (0..blocks * BLOCK)
            .map(|i| amp * (i as f32 * 2.0 * std::f32::consts::PI * 220.0 / RATE).sin())
            .collect()
    }

    fn noise(amp: f32, blocks: usize, mut seed: u32) -> Vec<f32> {
        (0..blocks * BLOCK)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                amp * ((seed >> 8) as f32 / 8_388_608.0 - 1.0)
            })
            .collect()
    }

    fn run(fold: &mut StereoFold, l: &[f32], r: &[f32]) -> Vec<f32> {
        let mut out = vec![0.0; l.len()];
        for ((l, r), o) in l
            .chunks(BLOCK)
            .zip(r.chunks(BLOCK))
            .zip(out.chunks_mut(BLOCK))
        {
            fold.process(l, r, o);
        }
        out
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn identical_channels_pass_unchanged() {
        let x = tone(0.3, 100);
        let mut fold = StereoFold::new(RATE);
        let out = run(&mut fold, &x, &x);
        assert_eq!(fold.mode(), FoldMode::Both);
        for (o, s) in out.iter().zip(&x) {
            assert!((o - s).abs() < 1e-6);
        }
    }

    #[test]
    fn one_dead_channel_is_not_halved() {
        let voice = tone(0.3, 200);
        let hiss = noise(1e-5, 200, 7);
        for flip in [false, true] {
            let mut fold = StereoFold::new(RATE);
            let out = if flip {
                run(&mut fold, &hiss, &voice)
            } else {
                run(&mut fold, &voice, &hiss)
            };
            assert_eq!(fold.mode(), FoldMode::Only(usize::from(flip)));
            let tail = out.len() - 50 * BLOCK;
            let ratio = rms(&out[tail..]) / rms(&voice[tail..]);
            assert!((ratio - 1.0).abs() < 0.01, "level ratio {ratio}");
        }
    }

    #[test]
    fn dead_channel_stays_dead_through_a_pause() {
        let mut fold = StereoFold::new(RATE);
        let hiss = noise(1e-5, 2000, 3);
        let voice = tone(0.3, 100);
        run(&mut fold, &voice, &hiss[..voice.len()]);
        assert_eq!(fold.mode(), FoldMode::Only(0));
        // Ten seconds of room noise on the live channel, hiss on the dead one.
        let room = noise(3e-4, 1875, 11);
        run(&mut fold, &room, &hiss[..room.len()]);
        assert_eq!(fold.mode(), FoldMode::Only(0));
    }

    #[test]
    fn real_stereo_microphone_is_averaged() {
        let l = tone(0.3, 200);
        let r: Vec<f32> = l.iter().map(|s| s * 0.4).collect();
        let mut fold = StereoFold::new(RATE);
        run(&mut fold, &l, &r);
        assert_eq!(fold.mode(), FoldMode::Both);
    }

    #[test]
    fn second_channel_coming_alive_is_used_again() {
        let voice = tone(0.3, 100);
        let hiss = noise(1e-5, 100, 5);
        let mut fold = StereoFold::new(RATE);
        run(&mut fold, &voice, &hiss);
        assert_eq!(fold.mode(), FoldMode::Only(0));
        run(&mut fold, &voice, &voice);
        assert_eq!(fold.mode(), FoldMode::Both);
    }

    #[test]
    fn opposite_polarity_does_not_cancel() {
        let l = tone(0.3, 200);
        let r: Vec<f32> = l.iter().map(|s| -s).collect();
        let mut fold = StereoFold::new(RATE);
        let out = run(&mut fold, &l, &r);
        assert_eq!(fold.mode(), FoldMode::Inverted);
        let tail = out.len() - 50 * BLOCK;
        let ratio = rms(&out[tail..]) / rms(&l[tail..]);
        assert!((ratio - 1.0).abs() < 0.01, "level ratio {ratio}");
    }

    #[test]
    fn silence_decides_nothing() {
        let z = vec![0.0; 100 * BLOCK];
        let mut fold = StereoFold::new(RATE);
        let out = run(&mut fold, &z, &z);
        assert_eq!(fold.mode(), FoldMode::Both);
        assert!(out.iter().all(|s| *s == 0.0));
    }
}
