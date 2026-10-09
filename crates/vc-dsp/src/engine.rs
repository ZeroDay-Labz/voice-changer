use crate::biquad::{Biquad, FilterKind};
use crate::denoise::Denoiser;
use crate::echo::Echo;
use crate::gate::NoiseGate;
use crate::leveler::Leveler;
use crate::params::{EngineParams, PitchMode};
use crate::pitch::PitchShifter;
use crate::reverb::Reverb;
use crate::ringmod::RingMod;
use crate::util::{DelayLine, Drive, Smooth, soft_limit};

/// Longest latency the engine will ever report; sizes the dry-path delay.
const MAX_LATENCY_SAMPLES: usize = 1 << 15;
/// Bypass crossfade: click-free yet instant-feeling on a hotkey.
const BYPASS_FADE_MS: f32 = 15.0;
const GAIN_SMOOTH_MS: f32 = 8.0;
const MAX_ECHO_MS: f32 = 1200.0;

/// The mono voice processing chain:
///
/// `in gain → denoise/VAD → gate → pitch/formant → drive → ring mod → echo → reverb → EQ → out gain → limiter`,
/// crossfaded against a latency-aligned dry copy when bypassed.
pub struct Engine {
    sample_rate: f32,
    params: EngineParams,
    in_gain: Smooth,
    out_gain: Smooth,
    denoise: Denoiser,
    leveler: Leveler,
    gate: NoiseGate,
    pitch: PitchShifter,
    drive: Drive,
    ring: RingMod,
    echo: Echo,
    reverb: Reverb,
    eq_low: Biquad,
    eq_mid: Biquad,
    eq_high: Biquad,
    /// 1.0 = fully wet (active), 0.0 = fully dry (bypassed).
    wet_mix: Smooth,
    dry_delay: DelayLine,
    scratch: Vec<f32>,
    latency_changed: bool,
    /// Peak absolute sample of the last processed block (pre / post chain).
    in_peak: f32,
    out_peak: f32,
}

impl Engine {
    /// `max_block` is the largest block `process` will ever be handed.
    pub fn new(sample_rate: f32, max_block: usize) -> Self {
        let max_block = max_block.max(1);
        let mut engine = Self {
            sample_rate,
            params: EngineParams::default(),
            in_gain: Smooth::new(1.0, GAIN_SMOOTH_MS, sample_rate),
            out_gain: Smooth::new(1.0, GAIN_SMOOTH_MS, sample_rate),
            denoise: Denoiser::new(sample_rate),
            leveler: Leveler::new(sample_rate),
            gate: NoiseGate::new(sample_rate),
            pitch: PitchShifter::new(sample_rate, max_block),
            drive: Drive::new(sample_rate),
            ring: RingMod::new(sample_rate),
            echo: Echo::new(sample_rate, MAX_ECHO_MS),
            reverb: Reverb::new(sample_rate),
            eq_low: Biquad::new(FilterKind::LowShelf, 200.0, 0.7, 0.0, sample_rate),
            eq_mid: Biquad::new(FilterKind::Peaking, 1000.0, 0.8, 0.0, sample_rate),
            eq_high: Biquad::new(FilterKind::HighShelf, 4000.0, 0.7, 0.0, sample_rate),
            wet_mix: Smooth::new(1.0, BYPASS_FADE_MS, sample_rate),
            dry_delay: DelayLine::new(MAX_LATENCY_SAMPLES),
            scratch: vec![0.0; max_block],
            latency_changed: false,
            in_peak: 0.0,
            out_peak: 0.0,
        };
        engine.set_params(EngineParams::default());
        engine
            .dry_delay
            .set_delay(engine.latency_samples() as usize);
        engine.latency_changed = false;
        engine
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Samples of delay the wet path adds. Hosts report this so the DAW can
    /// compensate; the dry path is delayed by the same amount so bypass
    /// toggles never shift timing.
    pub fn latency_samples(&self) -> u32 {
        let denoise = if self.params.denoise {
            Denoiser::latency_samples()
        } else {
            0
        };
        let pitch = if self.params.skip_voice_fx {
            0
        } else {
            self.pitch.latency_samples()
        };
        (pitch + denoise) as u32
    }

    /// Gain the auto-leveler is currently applying, in dB (0 when off).
    pub fn auto_gain_db(&self) -> f32 {
        if self.params.auto_level {
            self.leveler.gain_db()
        } else {
            0.0
        }
    }

    /// Speech probability from the noise suppressor (0 when it is off).
    pub fn vad(&self) -> f32 {
        if self.params.denoise {
            self.denoise.vad()
        } else {
            1.0
        }
    }

    /// True once after the latency changed (pitch mode switch); the host
    /// wrapper uses this to re-report latency.
    pub fn take_latency_changed(&mut self) -> bool {
        std::mem::take(&mut self.latency_changed)
    }

    /// Clear all internal state (on transport reset / stream restart).
    pub fn reset(&mut self) {
        self.dry_delay.clear();
        self.denoise.reset();
        self.leveler.reset();
        self.gate.reset();
        self.pitch.reset();
        self.ring.reset();
        self.echo.reset();
        self.reverb.reset();
        self.eq_low.clear();
        self.eq_mid.clear();
        self.eq_high.clear();
        self.in_gain.snap(self.params.input_gain);
        self.out_gain.snap(self.params.output_gain);
        self.wet_mix
            .snap(if self.params.bypass { 0.0 } else { 1.0 });
    }

    /// Hand the engine a new parameter snapshot. Realtime-safe.
    pub fn set_params(&mut self, p: EngineParams) {
        let denoise_changed =
            p.denoise != self.params.denoise || p.skip_voice_fx != self.params.skip_voice_fx;
        if p.skip_voice_fx != self.params.skip_voice_fx && !p.skip_voice_fx {
            // Coming back into the chain: start the stretcher clean.
            self.pitch.reset();
        }
        self.params = p;
        self.in_gain.set_target(p.input_gain);
        self.out_gain.set_target(p.output_gain);
        self.wet_mix.set_target(if p.bypass { 0.0 } else { 1.0 });
        self.gate.set_threshold_db(p.gate_threshold_db);
        self.denoise.set_floor_db(p.gate_floor_db);
        let mode_changed = self.pitch.set_mode(p.pitch_mode);
        if mode_changed || denoise_changed {
            self.dry_delay.set_delay(self.latency_samples() as usize);
            self.latency_changed = true;
        }
        self.pitch.set_shift(p.pitch_semitones, p.formant_semitones);
        self.drive.set_amount(p.drive);
        self.ring.set_freq(p.ring_freq_hz);
        self.ring.set_mix(p.ring_mix);
        self.echo.set(p.echo_time_ms, p.echo_feedback, p.echo_mix);
        self.reverb.set(p.reverb_size, p.reverb_damp, p.reverb_mix);
        self.eq_low.set_gain_db(p.eq_low_db);
        self.eq_mid.set_gain_db(p.eq_mid_db);
        self.eq_high.set_gain_db(p.eq_high_db);
    }

    pub fn params(&self) -> &EngineParams {
        &self.params
    }

    pub fn pitch_mode(&self) -> PitchMode {
        self.pitch.mode()
    }

    /// Peak levels (input, output) of the most recent block, for meters.
    pub fn peaks(&self) -> (f32, f32) {
        (self.in_peak, self.out_peak)
    }

    /// Process one mono block in place. Realtime-safe: no allocation, no locks.
    pub fn process(&mut self, buf: &mut [f32]) {
        self.process_with(buf, |_| {});
    }

    /// Like [`Self::process`], but runs `between` on the cleaned-up input
    /// (after gain, noise suppression, leveling and gating) and before the
    /// voice/effects chain. Hosts insert AI conversion there so the model
    /// gets clean speech and its output still passes through the effects.
    pub fn process_with(&mut self, buf: &mut [f32], mut between: impl FnMut(&mut [f32])) {
        self.in_peak = buf.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        // Larger blocks than promised are processed in slices rather than
        // allocating; this keeps the realtime guarantee intact.
        let chunk = self.scratch.len();
        let mut start = 0;
        while start < buf.len() {
            let end = (start + chunk).min(buf.len());
            self.process_chunk(&mut buf[start..end], &mut between);
            start = end;
        }
        self.out_peak = buf.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    }

    fn process_chunk(&mut self, buf: &mut [f32], between: &mut impl FnMut(&mut [f32])) {
        let n = buf.len();
        let dry = &mut self.scratch[..n];

        // Dry path: latency-aligned copy of the input.
        for (d, &x) in dry.iter_mut().zip(buf.iter()) {
            *d = self.dry_delay.process(x);
        }

        // Wet path, in place in `buf`.
        for x in buf.iter_mut() {
            *x *= self.in_gain.next();
        }
        if self.params.denoise {
            self.denoise.process(buf, self.params.voice_only);
        }
        if self.params.auto_level {
            // Without the denoiser's VAD, treat everything as speech.
            let speech = !self.params.denoise || self.denoise.vad() > 0.5;
            self.leveler.process(buf, speech);
        }
        if self.params.gate_enabled {
            self.gate.process(buf);
        }
        between(buf);
        if !self.params.skip_voice_fx {
            self.pitch.process(buf);
            self.drive.process(buf);
            self.ring.process(buf);
            self.echo.process(buf);
            self.reverb.process(buf);
            if !self.eq_low.is_identity() {
                self.eq_low.process(buf);
            }
            if !self.eq_mid.is_identity() {
                self.eq_mid.process(buf);
            }
            if !self.eq_high.is_identity() {
                self.eq_high.process(buf);
            }
        }
        for x in buf.iter_mut() {
            *x *= self.out_gain.next();
        }
        if self.params.limiter_enabled {
            for x in buf.iter_mut() {
                *x = soft_limit(*x);
            }
        }

        // Bypass crossfade (equal-power so loudness doesn't dip mid-fade).
        if self.wet_mix.is_settled() {
            if self.wet_mix.current() < 0.5 {
                buf.copy_from_slice(dry);
            }
            return;
        }
        for (x, &d) in buf.iter_mut().zip(dry.iter()) {
            let g = self.wet_mix.next();
            let theta = g * core::f32::consts::FRAC_PI_2;
            let (wet_w, dry_w) = theta.sin_cos();
            *x = *x * wet_w + d * dry_w;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn neutral() -> EngineParams {
        EngineParams {
            pitch_mode: PitchMode::Off,
            limiter_enabled: false,
            denoise: false,
            voice_only: false,
            auto_level: false,
            ..Default::default()
        }
    }

    #[test]
    fn passthrough_when_everything_is_neutral() {
        let mut e = Engine::new(48_000.0, 256);
        e.set_params(neutral());
        e.reset();
        let mut buf: Vec<f32> = (0..256).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
        let orig = buf.clone();
        e.process(&mut buf);
        for (a, b) in buf.iter().zip(orig.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn gain_applies_after_smoothing() {
        let mut e = Engine::new(48_000.0, 512);
        e.set_params(EngineParams {
            input_gain: 0.5,
            ..neutral()
        });
        let mut buf = vec![1.0f32; 4096];
        e.process(&mut buf);
        assert!((buf[4095] - 0.5).abs() < 1e-3, "got {}", buf[4095]);
    }

    #[test]
    fn bypass_is_click_free_and_time_aligned() {
        let mut e = Engine::new(48_000.0, 512);
        // Balanced mode adds 2048 samples of latency; dry must be delayed to match.
        e.set_params(EngineParams {
            output_gain: 0.0,
            pitch_mode: PitchMode::Balanced,
            denoise: false,
            auto_level: false,
            ..Default::default()
        });
        e.reset();
        assert_eq!(e.latency_samples(), 2048);
        let mut buf = vec![1.0f32; 4096];
        e.process(&mut buf); // settle at silence, prime the dry delay
        e.set_params(EngineParams {
            output_gain: 0.0,
            bypass: true,
            pitch_mode: PitchMode::Balanced,
            denoise: false,
            auto_level: false,
            ..Default::default()
        });
        let mut buf = vec![1.0f32; 8192];
        e.process(&mut buf);
        let max_jump = buf
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(max_jump < 0.01, "max jump {max_jump}");
        assert!((buf[8191] - 1.0).abs() < 1e-3);
    }

    #[test]
    fn switching_pitch_mode_flags_latency_change() {
        let mut e = Engine::new(48_000.0, 256);
        assert!(!e.take_latency_changed());
        e.set_params(EngineParams {
            pitch_mode: PitchMode::LowLatency,
            denoise: false,
            ..Default::default()
        });
        assert!(e.take_latency_changed());
        assert_eq!(e.latency_samples(), 1024);
        assert!(!e.take_latency_changed());
    }

    #[test]
    fn full_chain_produces_finite_output() {
        let mut e = Engine::new(48_000.0, 256);
        e.set_params(EngineParams {
            gate_enabled: true,
            pitch_semitones: -7.0,
            formant_semitones: -3.0,
            drive: 0.6,
            ring_mix: 0.5,
            echo_mix: 0.4,
            reverb_mix: 0.4,
            eq_low_db: 6.0,
            eq_mid_db: -3.0,
            eq_high_db: 4.0,
            input_gain: 4.0,
            ..Default::default()
        });
        let mut buf: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * std::f32::consts::PI * 150.0 * i as f32 / 48_000.0).sin())
            .collect();
        for chunk in buf.chunks_mut(256) {
            e.process(chunk);
        }
        assert!(buf.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
    }
}
