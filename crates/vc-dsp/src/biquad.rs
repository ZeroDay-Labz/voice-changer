//! RBJ "cookbook" biquad filters.

use std::f32::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FilterKind {
    LowShelf,
    Peaking,
    HighShelf,
    LowPass,
    HighPass,
}

#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
    // Cached design so we can skip recomputation when nothing changed.
    kind: FilterKind,
    freq: f32,
    q: f32,
    gain_db: f32,
    sample_rate: f32,
}

impl Biquad {
    pub fn new(kind: FilterKind, freq: f32, q: f32, gain_db: f32, sample_rate: f32) -> Self {
        let mut f = Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
            kind,
            freq,
            q,
            gain_db,
            sample_rate,
        };
        f.design();
        f
    }

    /// Update the gain; recomputes coefficients only if it changed.
    #[inline]
    pub fn set_gain_db(&mut self, gain_db: f32) {
        if (gain_db - self.gain_db).abs() > 1e-4 {
            self.gain_db = gain_db;
            self.design();
        }
    }

    pub fn set_freq(&mut self, freq: f32) {
        if (freq - self.freq).abs() > 1e-3 {
            self.freq = freq;
            self.design();
        }
    }

    pub fn clear(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    pub fn is_identity(&self) -> bool {
        matches!(
            self.kind,
            FilterKind::LowShelf | FilterKind::Peaking | FilterKind::HighShelf
        ) && self.gain_db.abs() < 1e-4
    }

    fn design(&mut self) {
        let a = 10f32.powf(self.gain_db / 40.0);
        let w0 = 2.0 * PI * (self.freq / self.sample_rate).clamp(1e-5, 0.499);
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * self.q.max(0.05));
        let (b0, b1, b2, a0, a1, a2) = match self.kind {
            FilterKind::Peaking => (
                1.0 + alpha * a,
                -2.0 * cos,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos,
                1.0 - alpha / a,
            ),
            FilterKind::LowShelf => {
                let sa = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cos + sa),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                    a * ((a + 1.0) - (a - 1.0) * cos - sa),
                    (a + 1.0) + (a - 1.0) * cos + sa,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                    (a + 1.0) + (a - 1.0) * cos - sa,
                )
            }
            FilterKind::HighShelf => {
                let sa = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cos + sa),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                    a * ((a + 1.0) + (a - 1.0) * cos - sa),
                    (a + 1.0) - (a - 1.0) * cos + sa,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos),
                    (a + 1.0) - (a - 1.0) * cos - sa,
                )
            }
            FilterKind::LowPass => (
                (1.0 - cos) / 2.0,
                1.0 - cos,
                (1.0 - cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            FilterKind::HighPass => (
                (1.0 + cos) / 2.0,
                -(1.0 + cos),
                (1.0 + cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
        };
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    /// Transposed direct form II.
    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        for x in buf.iter_mut() {
            *x = self.tick(*x);
        }
    }

    /// Filter `input` into `out` (resized to match). Not realtime-safe
    /// (may allocate on growth); for worker threads.
    pub fn process_into(&mut self, input: &[f32], out: &mut Vec<f32>) {
        out.clear();
        out.extend(input.iter().map(|&x| self.tick(x)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms_of_sine(f: &mut Biquad, hz: f32, rate: f32) -> f32 {
        let n = (rate as usize) / 2;
        let mut acc = 0.0;
        for i in 0..n {
            let x = (2.0 * PI * hz * i as f32 / rate).sin();
            let y = f.tick(x);
            if i > n / 2 {
                acc += y * y;
            }
        }
        (acc / (n / 2) as f32).sqrt()
    }

    #[test]
    fn peaking_boosts_center_only() {
        let rate = 48_000.0;
        let mut f = Biquad::new(FilterKind::Peaking, 1000.0, 1.0, 12.0, rate);
        let at_center = rms_of_sine(&mut f, 1000.0, rate);
        f.clear();
        let far = rms_of_sine(&mut f, 100.0, rate);
        let unity = 1.0 / 2f32.sqrt();
        assert!(
            (at_center / unity) > 3.5,
            "center gain {}",
            at_center / unity
        );
        assert!((far / unity - 1.0).abs() < 0.1, "far gain {}", far / unity);
    }

    #[test]
    fn zero_gain_shelf_is_identity() {
        let f = Biquad::new(FilterKind::LowShelf, 200.0, 0.7, 0.0, 48_000.0);
        assert!(f.is_identity());
    }
}
