//! Noise gate with hysteresis and hold.

use crate::util::db_to_gain;

#[derive(Debug, Clone, Copy)]
pub struct NoiseGate {
    open_threshold: f32,
    close_threshold: f32,
    env: f32,
    env_attack: f32,
    env_release: f32,
    gain: f32,
    gain_attack: f32,
    gain_release: f32,
    hold_samples: u32,
    hold_left: u32,
    open: bool,
}

impl NoiseGate {
    pub fn new(sample_rate: f32) -> Self {
        let coeff = |ms: f32| (-1.0 / (ms * 0.001 * sample_rate).max(1.0)).exp();
        let mut g = Self {
            open_threshold: 0.0,
            close_threshold: 0.0,
            env: 0.0,
            env_attack: coeff(0.5),
            env_release: coeff(40.0),
            gain: 1.0,
            gain_attack: coeff(2.0),
            gain_release: coeff(80.0),
            hold_samples: (0.12 * sample_rate) as u32,
            hold_left: 0,
            open: true,
        };
        g.set_threshold_db(-50.0);
        g
    }

    /// Open at `threshold`, close 6 dB below it so it doesn't chatter.
    pub fn set_threshold_db(&mut self, threshold_db: f32) {
        self.open_threshold = db_to_gain(threshold_db);
        self.close_threshold = db_to_gain(threshold_db - 6.0);
    }

    pub fn reset(&mut self) {
        self.env = 0.0;
        self.gain = 1.0;
        self.open = true;
        self.hold_left = 0;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        let level = x.abs();
        let coeff = if level > self.env {
            self.env_attack
        } else {
            self.env_release
        };
        self.env = level + coeff * (self.env - level);

        if self.env > self.open_threshold {
            self.open = true;
            self.hold_left = self.hold_samples;
        } else if self.env < self.close_threshold {
            if self.hold_left > 0 {
                self.hold_left -= 1;
            } else {
                self.open = false;
            }
        }

        let target = if self.open { 1.0 } else { 0.0 };
        let coeff = if target > self.gain {
            self.gain_attack
        } else {
            self.gain_release
        };
        self.gain = target + coeff * (self.gain - target);
        x * self.gain
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        for x in buf.iter_mut() {
            *x = self.tick(*x);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closes_on_silence_and_reopens_on_signal() {
        let rate = 48_000.0;
        let mut g = NoiseGate::new(rate);
        g.set_threshold_db(-30.0);
        // Quiet noise well below threshold for 400 ms (past the hold time).
        let mut last = 1.0;
        for i in 0..(0.4 * rate) as usize {
            let x = 0.001 * ((i % 7) as f32 - 3.0);
            last = g.tick(x);
        }
        assert!(!g.is_open());
        assert!(last.abs() < 1e-4);
        // Loud signal reopens.
        for i in 0..2000 {
            g.tick(0.5 * (i as f32 * 0.1).sin());
        }
        assert!(g.is_open());
    }
}
