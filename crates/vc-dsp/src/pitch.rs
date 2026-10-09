//! Pitch and formant shifting via Signalsmith Stretch, with selectable
//! latency/quality trade-off. All configurations are allocated up front so
//! switching between them is realtime-safe.

use crate::params::PitchMode;
use crate::psola::Psola;
use signalsmith_stretch::Stretch;

struct Config {
    mode: PitchMode,
    stretch: Stretch,
    latency: usize,
}

pub struct PitchShifter {
    configs: Vec<Config>,
    psola: Psola,
    natural: bool,
    active: Option<usize>,
    pitch_semitones: f32,
    formant_semitones: f32,
    tonality_limit: f32,
    scratch: Vec<f32>,
}

impl PitchShifter {
    pub fn new(sample_rate: f32, max_block: usize) -> Self {
        let mut configs = Vec::new();
        for mode in PitchMode::ALL {
            if let Some((block, interval)) = mode.block_interval(sample_rate) {
                let stretch = Stretch::new(1, block, interval);
                let latency = stretch.input_latency() + stretch.output_latency();
                configs.push(Config {
                    mode,
                    stretch,
                    latency,
                });
            }
        }
        let mut s = Self {
            configs,
            psola: Psola::new(sample_rate),
            natural: false,
            active: None,
            pitch_semitones: 0.0,
            formant_semitones: 0.0,
            // Treat everything above 8 kHz as noise-like (keeps consonants crisp).
            tonality_limit: 8_000.0 / sample_rate,
            scratch: vec![0.0; max_block.max(1)],
        };
        s.set_mode(PitchMode::Balanced);
        s
    }

    pub fn mode(&self) -> PitchMode {
        if self.natural {
            return PitchMode::Natural;
        }
        self.active
            .map(|i| self.configs[i].mode)
            .unwrap_or(PitchMode::Off)
    }

    /// Switch configuration. Returns true if the latency changed.
    pub fn set_mode(&mut self, mode: PitchMode) -> bool {
        let before = self.latency_samples();
        let natural = mode == PitchMode::Natural;
        if natural != self.natural {
            self.natural = natural;
            if natural {
                self.psola.reset();
            }
        }
        let new = if natural {
            None
        } else {
            self.configs.iter().position(|c| c.mode == mode)
        };
        if new != self.active {
            self.active = new;
            if let Some(i) = self.active {
                self.configs[i].stretch.reset();
            }
        }
        self.apply_factors();
        before != self.latency_samples()
    }

    pub fn latency_samples(&self) -> usize {
        if self.natural {
            return self.psola.latency_samples();
        }
        self.active.map(|i| self.configs[i].latency).unwrap_or(0)
    }

    pub fn set_shift(&mut self, pitch_semitones: f32, formant_semitones: f32) {
        if (pitch_semitones - self.pitch_semitones).abs() > 1e-4
            || (formant_semitones - self.formant_semitones).abs() > 1e-4
        {
            self.pitch_semitones = pitch_semitones;
            self.formant_semitones = formant_semitones;
            self.apply_factors();
        }
    }

    fn apply_factors(&mut self) {
        self.psola
            .set_shift(self.pitch_semitones, self.formant_semitones);
        let Some(i) = self.active else { return };
        let s = &mut self.configs[i].stretch;
        s.set_transpose_factor_semitones(self.pitch_semitones, Some(self.tonality_limit));
        // compensate_pitch = true: formant 0 keeps the speaker's natural
        // formants in place while the pitch moves (the "voice changer" feel);
        // non-zero values then shift them relative to that.
        s.set_formant_factor_semitones(self.formant_semitones, true);
    }

    pub fn reset(&mut self) {
        self.psola.reset();
        if let Some(i) = self.active {
            self.configs[i].stretch.reset();
        }
    }

    /// Process in place. Realtime-safe for blocks up to `max_block`.
    pub fn process(&mut self, buf: &mut [f32]) {
        if self.natural {
            self.psola.process(buf);
            return;
        }
        let Some(i) = self.active else { return };
        let stretch = &mut self.configs[i].stretch;
        let chunk = self.scratch.len();
        let mut start = 0;
        while start < buf.len() {
            let end = (start + chunk).min(buf.len());
            let n = end - start;
            let out = &mut self.scratch[..n];
            stretch.process(&buf[start..end], &mut *out);
            buf[start..end].copy_from_slice(out);
            start = end;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    fn zero_cross_freq(x: &[f32]) -> f32 {
        let crossings = x
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        crossings as f32 / 2.0 / (x.len() as f32 / RATE)
    }

    #[test]
    fn octave_up_doubles_frequency() {
        let mut p = PitchShifter::new(RATE, 256);
        p.set_mode(PitchMode::Balanced);
        p.set_shift(12.0, 0.0);
        let mut buf: Vec<f32> = (0..RATE as usize)
            .map(|i| (2.0 * std::f32::consts::PI * 220.0 * i as f32 / RATE).sin())
            .collect();
        for chunk in buf.chunks_mut(256) {
            p.process(chunk);
        }
        let f = zero_cross_freq(&buf[buf.len() / 2..]);
        assert!((f - 440.0).abs() / 440.0 < 0.03, "measured {f}");
    }

    #[test]
    fn latencies_match_expectations() {
        let mut p = PitchShifter::new(RATE, 256);
        p.set_mode(PitchMode::LowLatency);
        assert_eq!(p.latency_samples(), 1024);
        assert!(p.set_mode(PitchMode::Balanced));
        assert_eq!(p.latency_samples(), 2048);
        p.set_mode(PitchMode::HighQuality);
        assert_eq!(p.latency_samples(), 4096);
        p.set_mode(PitchMode::Off);
        assert_eq!(p.latency_samples(), 0);
    }
}
