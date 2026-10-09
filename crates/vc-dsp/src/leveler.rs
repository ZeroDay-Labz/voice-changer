//! Automatic level control: slow RMS-based gain riding that keeps speech
//! near a target loudness so the user never has to chase the input knob,
//! followed by the soft limiter for anything that still peaks.

use crate::util::{db_to_gain, gain_to_db};

pub struct Leveler {
    target_rms: f32,
    rms_sq: f32,
    rms_coeff: f32,
    gain_db: f32,
    gain: f32,
    up_per_sample: f32,
    down_per_sample: f32,
    min_db: f32,
    max_db: f32,
    gate_rms: f32,
}

impl Leveler {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            // Speech sitting around -18 dBFS RMS peaks comfortably below 0.
            target_rms: db_to_gain(-18.0),
            rms_sq: 0.0,
            rms_coeff: (-1.0 / (0.25 * sample_rate)).exp(),
            gain_db: 0.0,
            gain: 1.0,
            // Gain rises slowly (2 s for 6 dB) and comes down fast (150 ms for 6 dB).
            up_per_sample: 6.0 / (2.0 * sample_rate),
            down_per_sample: 6.0 / (0.15 * sample_rate),
            min_db: -12.0,
            max_db: 24.0,
            // Below this the input is noise/silence: hold the gain, don't crank it.
            gate_rms: db_to_gain(-40.0),
        }
    }

    pub fn reset(&mut self) {
        self.rms_sq = 0.0;
        self.gain_db = 0.0;
        self.gain = 1.0;
    }

    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }

    /// `speech`: whether a voice detector currently hears speech. Gain only
    /// moves during speech, so pauses and room noise are never pulled up.
    pub fn process(&mut self, buf: &mut [f32], speech: bool) {
        for x in buf.iter_mut() {
            self.rms_sq = *x * *x + self.rms_coeff * (self.rms_sq - *x * *x);
            let rms = self.rms_sq.sqrt();
            if speech && rms > self.gate_rms {
                let wanted = gain_to_db(self.target_rms) - gain_to_db(rms);
                let step = if wanted > self.gain_db {
                    self.up_per_sample
                } else {
                    -self.down_per_sample
                };
                let next = (self.gain_db + step).clamp(self.min_db, self.max_db);
                // Don't overshoot the wanted gain.
                self.gain_db = if step > 0.0 {
                    next.min(wanted.max(self.min_db))
                } else {
                    next.max(wanted.min(self.max_db))
                };
                self.gain = db_to_gain(self.gain_db);
            }
            *x *= self.gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brings_quiet_speech_up_and_loud_down() {
        let sr = 48_000.0;
        let mut l = Leveler::new(sr);
        let quiet: Vec<f32> = (0..(sr as usize * 6))
            .map(|i| 0.02 * (2.0 * std::f32::consts::PI * 200.0 * i as f32 / sr).sin())
            .collect();
        let mut y = quiet.clone();
        for chunk in y.chunks_mut(256) {
            l.process(chunk, true);
        }
        let tail = &y[y.len() - sr as usize..];
        let rms = (tail.iter().map(|v| v * v).sum::<f32>() / tail.len() as f32).sqrt();
        assert!(
            (gain_to_db(rms) - (-18.0)).abs() < 3.0,
            "rms {} dB",
            gain_to_db(rms)
        );

        let loud: Vec<f32> = (0..(sr as usize * 2))
            .map(|i| 0.9 * (2.0 * std::f32::consts::PI * 200.0 * i as f32 / sr).sin())
            .collect();
        let mut y = loud.clone();
        for chunk in y.chunks_mut(256) {
            l.process(chunk, true);
        }
        let tail = &y[y.len() - sr as usize / 2..];
        let rms = (tail.iter().map(|v| v * v).sum::<f32>() / tail.len() as f32).sqrt();
        assert!(gain_to_db(rms) < -12.0, "rms {} dB", gain_to_db(rms));
    }
}
