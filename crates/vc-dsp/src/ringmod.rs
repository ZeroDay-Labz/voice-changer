//! Ring modulator: the classic "robot"/Dalek effect.

use std::f32::consts::TAU;

#[derive(Debug, Clone, Copy)]
pub struct RingMod {
    phase: f32,
    inc: f32,
    sample_rate: f32,
    mix: f32,
}

impl RingMod {
    pub fn new(sample_rate: f32) -> Self {
        let mut r = Self {
            phase: 0.0,
            inc: 0.0,
            sample_rate,
            mix: 0.0,
        };
        r.set_freq(80.0);
        r
    }

    pub fn set_freq(&mut self, hz: f32) {
        self.inc = TAU * hz / self.sample_rate;
    }

    pub fn set_mix(&mut self, mix: f32) {
        self.mix = mix.clamp(0.0, 1.0);
    }

    pub fn reset(&mut self) {
        self.phase = 0.0;
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        if self.mix <= 0.0 {
            return;
        }
        let dry = 1.0 - self.mix;
        for x in buf.iter_mut() {
            let carrier = self.phase.sin();
            self.phase += self.inc;
            if self.phase >= TAU {
                self.phase -= TAU;
            }
            *x = *x * dry + *x * carrier * self.mix;
        }
    }
}
