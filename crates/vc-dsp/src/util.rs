/// Convert decibels to a linear gain factor.
#[inline]
pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Convert a linear gain factor to decibels, clamping silence to -inf-ish.
#[inline]
pub fn gain_to_db(gain: f32) -> f32 {
    if gain <= 1e-9 {
        -180.0
    } else {
        20.0 * gain.log10()
    }
}

/// One-pole smoother for control signals. Cheap, allocation-free.
#[derive(Debug, Clone, Copy)]
pub struct Smooth {
    value: f32,
    target: f32,
    coeff: f32,
}

impl Smooth {
    /// `time_ms` is the time constant: after that long the value has moved
    /// ~63% of the way to the target.
    pub fn new(initial: f32, time_ms: f32, sample_rate: f32) -> Self {
        let mut s = Self {
            value: initial,
            target: initial,
            coeff: 0.0,
        };
        s.set_time(time_ms, sample_rate);
        s
    }

    pub fn set_time(&mut self, time_ms: f32, sample_rate: f32) {
        let samples = (time_ms * 0.001 * sample_rate).max(1.0);
        self.coeff = (-1.0 / samples).exp();
    }

    #[inline]
    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Jump straight to a value (used on reset).
    pub fn snap(&mut self, value: f32) {
        self.value = value;
        self.target = value;
    }

    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f32 {
        self.value = self.target + self.coeff * (self.value - self.target);
        self.value
    }

    #[inline]
    pub fn current(&self) -> f32 {
        self.value
    }

    #[inline]
    pub fn is_settled(&self) -> bool {
        (self.value - self.target).abs() < 1e-6
    }
}

/// Fixed-size delay line used to time-align the dry path with the wet path.
#[derive(Debug, Clone)]
pub struct DelayLine {
    buf: Vec<f32>,
    write: usize,
    delay: usize,
}

impl DelayLine {
    /// Allocates for `max_delay` samples. Allocation happens here, never in `process`.
    pub fn new(max_delay: usize) -> Self {
        Self {
            buf: vec![0.0; max_delay.max(1)],
            write: 0,
            delay: 0,
        }
    }

    pub fn set_delay(&mut self, delay: usize) {
        self.delay = delay.min(self.buf.len() - 1);
    }

    pub fn delay(&self) -> usize {
        self.delay
    }

    pub fn clear(&mut self) {
        self.buf.fill(0.0);
        self.write = 0;
    }

    #[inline]
    pub fn process(&mut self, input: f32) -> f32 {
        if self.delay == 0 {
            return input;
        }
        let len = self.buf.len();
        let read = (self.write + len - self.delay) % len;
        let out = self.buf[read];
        self.buf[self.write] = input;
        self.write = (self.write + 1) % len;
        out
    }
}

/// Tube-ish saturation with level compensation so turning it up adds grit,
/// not just volume. `drive` is 0..1.
#[derive(Debug, Clone, Copy)]
pub struct Drive {
    pre: Smooth,
}

impl Drive {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            pre: Smooth::new(1.0, 20.0, sample_rate),
        }
    }

    pub fn set_amount(&mut self, drive: f32) {
        self.pre.set_target(1.0 + drive.clamp(0.0, 1.0) * 24.0);
    }

    pub fn is_bypassed(&self) -> bool {
        self.pre.is_settled() && self.pre.current() <= 1.0 + 1e-3
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        if self.is_bypassed() {
            return;
        }
        for x in buf.iter_mut() {
            let pre = self.pre.next();
            // Normalize so a -6 dBFS input stays about -6 dBFS.
            let comp = 0.5 / (0.5 * pre).tanh();
            *x = (*x * pre).tanh() * comp;
        }
    }
}

/// Transparent below the knee, smoothly saturating above it. Keeps the
/// virtual mic from clipping when someone stacks +24 dB of gain and reverb.
#[inline]
pub fn soft_limit(x: f32) -> f32 {
    const KNEE: f32 = 0.8;
    let a = x.abs();
    if a <= KNEE {
        x
    } else {
        let over = (a - KNEE) / (1.0 - KNEE);
        x.signum() * (KNEE + (1.0 - KNEE) * over.tanh())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_roundtrip() {
        for db in [-24.0, -6.0, 0.0, 6.0, 12.0] {
            assert!((gain_to_db(db_to_gain(db)) - db).abs() < 1e-3);
        }
    }

    #[test]
    fn delay_line_delays() {
        let mut d = DelayLine::new(16);
        d.set_delay(3);
        let out: Vec<f32> = (1..=6).map(|i| d.process(i as f32)).collect();
        assert_eq!(out, vec![0.0, 0.0, 0.0, 1.0, 2.0, 3.0]);
    }

    #[test]
    fn smoother_converges() {
        let mut s = Smooth::new(0.0, 10.0, 48_000.0);
        s.set_target(1.0);
        for _ in 0..48_00 {
            s.next();
        }
        assert!(s.is_settled() || (s.current() - 1.0).abs() < 1e-3);
    }
}
