//! Feedback delay ("echo") with a damping low-pass in the loop.

use crate::biquad::{Biquad, FilterKind};
use crate::util::Smooth;

pub struct Echo {
    buf: Vec<f32>,
    write: usize,
    delay: Smooth,
    feedback: f32,
    mix: f32,
    damp: Biquad,
    sample_rate: f32,
}

impl Echo {
    pub fn new(sample_rate: f32, max_ms: f32) -> Self {
        let len = ((max_ms * 0.001 * sample_rate) as usize).max(2);
        Self {
            buf: vec![0.0; len],
            write: 0,
            delay: Smooth::new(0.25 * sample_rate, 30.0, sample_rate),
            feedback: 0.3,
            mix: 0.0,
            damp: Biquad::new(FilterKind::LowPass, 3500.0, 0.7, 0.0, sample_rate),
            sample_rate,
        }
    }

    pub fn set(&mut self, time_ms: f32, feedback: f32, mix: f32) {
        let samples = (time_ms * 0.001 * self.sample_rate).clamp(1.0, (self.buf.len() - 2) as f32);
        self.delay.set_target(samples);
        self.feedback = feedback.clamp(0.0, 0.95);
        self.mix = mix.clamp(0.0, 1.0);
    }

    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        self.write = 0;
        self.damp.clear();
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        if self.mix <= 0.0 && self.buf.iter().take(64).all(|&v| v == 0.0) {
            // Nothing wet to output and no tail ringing: skip.
            self.delay.next();
            return;
        }
        let len = self.buf.len();
        for x in buf.iter_mut() {
            // Fractional read with linear interpolation so time changes glide.
            let d = self.delay.next();
            let read_pos = (self.write as f32 + len as f32 - d) % len as f32;
            let i0 = read_pos as usize;
            let frac = read_pos - i0 as f32;
            let i1 = (i0 + 1) % len;
            let wet = self.buf[i0] * (1.0 - frac) + self.buf[i1] * frac;

            let fb = self.damp.tick(wet) * self.feedback;
            self.buf[self.write] = *x + fb;
            self.write = (self.write + 1) % len;

            *x = *x * (1.0 - self.mix) + wet * self.mix;
        }
    }
}
