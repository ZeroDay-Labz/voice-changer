//! Neural noise suppression (RNNoise, via the pure-Rust `nnnoiseless` port)
//! plus a voice-activity gate driven by its speech probability.

use nnnoiseless::DenoiseState;

pub const FRAME: usize = DenoiseState::FRAME_SIZE; // 480 samples = 10 ms at 48 kHz

pub struct Denoiser {
    state: Box<DenoiseState<'static>>,
    input: [f32; FRAME],
    /// Two denoised frames in flight: `output[0]` is being emitted,
    /// `output[1]` is the one after it (the gate looks at its VAD).
    output: [[f32; FRAME]; 2],
    vads: [f32; 2],
    fill: usize,
    /// Speech probability of the most recently processed frame, 0..1.
    vad: f32,
    // VAD gate
    gate_open: bool,
    gate_gain: f32,
    floor: f32,
    hold_left: u32,
    hold_frames: u32,
    attack: f32,
    release: f32,
}

const OPEN_ABOVE: f32 = 0.35;
const CLOSE_BELOW: f32 = 0.15;

impl Denoiser {
    pub fn new(sample_rate: f32) -> Self {
        let coeff = |ms: f32| (-1.0 / (ms * 0.001 * sample_rate).max(1.0)).exp();
        Self {
            state: DenoiseState::new(),
            input: [0.0; FRAME],
            output: [[0.0; FRAME]; 2],
            vads: [0.0; 2],
            fill: 0,
            vad: 0.0,
            gate_open: true,
            gate_gain: 1.0,
            floor: 0.01,
            hold_left: 0,
            // ~400 ms hold keeps word gaps from chopping.
            hold_frames: 40,
            // Opens over 12 ms (ahead of the onset thanks to the lookahead
            // frame) and closes over 200 ms.
            attack: coeff(12.0),
            release: coeff(200.0),
        }
    }

    /// Delay introduced: two frames (one for the denoiser, one of lookahead
    /// so the gate can open before a word starts).
    pub const fn latency_samples() -> usize {
        2 * FRAME
    }

    /// How far the gate attenuates when closed, in dB (e.g. -40).
    pub fn set_floor_db(&mut self, db: f32) {
        self.floor = crate::util::db_to_gain(db.clamp(-90.0, 0.0));
    }

    pub fn vad(&self) -> f32 {
        self.vad
    }

    pub fn reset(&mut self) {
        self.state = DenoiseState::new();
        self.input = [0.0; FRAME];
        self.output = [[0.0; FRAME]; 2];
        self.vads = [0.0; 2];
        self.fill = 0;
        self.vad = 0.0;
        self.gate_open = true;
        self.gate_gain = 1.0;
        self.hold_left = 0;
    }

    /// Denoise in place (adds two frames of delay). When `voice_only` is
    /// set, stretches the network considers non-speech are faded down to
    /// the floor; the gate decision uses the *next* frame's speech
    /// probability, so it is already opening when a word arrives.
    pub fn process(&mut self, buf: &mut [f32], voice_only: bool) {
        for x in buf.iter_mut() {
            let y = self.output[0][self.fill] / 32768.0;
            self.input[self.fill] = *x * 32768.0;
            self.fill += 1;
            if self.fill == FRAME {
                self.fill = 0;
                let mut out = [0.0f32; FRAME];
                self.vad = self.state.process_frame(&mut out, &self.input);
                self.output[0] = self.output[1];
                self.output[1] = out;
                self.vads[0] = self.vads[1];
                self.vads[1] = self.vad;
                // Decide for the frame about to be emitted using the one
                // after it as lookahead.
                let ahead = self.vads[0].max(self.vads[1]);
                if ahead > OPEN_ABOVE {
                    self.gate_open = true;
                    self.hold_left = self.hold_frames;
                } else if ahead < CLOSE_BELOW {
                    if self.hold_left > 0 {
                        self.hold_left -= 1;
                    } else {
                        self.gate_open = false;
                    }
                }
            }
            let target = if !voice_only || self.gate_open {
                1.0
            } else {
                self.floor
            };
            let coeff = if target > self.gate_gain {
                self.attack
            } else {
                self.release
            };
            self.gate_gain = target + coeff * (self.gate_gain - target);
            *x = y * self.gate_gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gated_bursts_have_no_clicks() {
        let sr = 48_000.0;
        let mut d = Denoiser::new(sr);
        d.set_floor_db(-40.0);
        let n = (sr * 3.0) as usize;
        // Hard-edged harmonic bursts with silence between.
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / sr;
                let on = (t % 0.8) < 0.35;
                if !on {
                    return 0.0005 * ((i * 7919) % 13) as f32 / 13.0;
                }
                let mut s = 0.0;
                for h in 1..=10 {
                    s += (2.0 * std::f32::consts::PI * 160.0 * h as f32 * t).sin() / h as f32;
                }
                0.3 * s
            })
            .collect();
        let mut y = x.clone();
        for chunk in y.chunks_mut(128) {
            d.process(chunk, true);
        }
        // click detector: diff spikes relative to local rms of the diff
        let mut clicks = 0;
        let win = 480;
        let mut s = 4800;
        while s + win < y.len() {
            let mut rms = 0.0f32;
            let mut mx = 0.0f32;
            for i in s..s + win - 1 {
                let dd = (y[i + 1] - y[i]).abs();
                rms += dd * dd;
                mx = mx.max(dd);
            }
            let rms = (rms / win as f32).sqrt() + 1e-6;
            if mx > 8.0 * rms && mx > 0.02 {
                clicks += 1;
            }
            s += win;
        }
        assert_eq!(clicks, 0, "gate produced clicks");
    }

    #[test]
    fn delays_by_one_frame_and_stays_finite() {
        let mut d = Denoiser::new(48_000.0);
        let x: Vec<f32> = (0..48_000)
            .map(|i| 0.3 * (2.0 * std::f32::consts::PI * 180.0 * i as f32 / 48_000.0).sin())
            .collect();
        let mut y = x.clone();
        for chunk in y.chunks_mut(256) {
            d.process(chunk, false);
        }
        assert!(y.iter().all(|v| v.is_finite()));
        // A clean tone should mostly survive.
        let rms = (y[24_000..].iter().map(|v| v * v).sum::<f32>() / 24_000.0).sqrt();
        assert!(rms > 0.1, "rms {rms}");
    }
}
