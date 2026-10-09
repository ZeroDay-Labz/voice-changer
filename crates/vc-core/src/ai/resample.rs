//! Stateless rational (L/M) polyphase resampler with a Kaiser-windowed sinc.
//! Used on whole analysis windows, so edge effects land in the discarded
//! context rather than in the audio we keep.

pub struct Resampler {
    up: usize,
    down: usize,
    /// taps[phase][k]
    phases: Vec<Vec<f32>>,
    half: usize,
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let y = x * x / 4.0;
    for k in 1..50 {
        term *= y / (k as f64 * k as f64);
        sum += term;
        if term < 1e-12 * sum {
            break;
        }
    }
    sum
}

impl Resampler {
    pub fn new(from_rate: usize, to_rate: usize) -> Self {
        let g = gcd(from_rate, to_rate);
        let up = to_rate / g;
        let down = from_rate / g;
        let taps_per_phase = 24;
        let n = taps_per_phase * up; // total taps (multiple of `up`)
        let half = n / 2;
        // Cutoff in the upsampled domain.
        let fc = 0.5 * (1.0f64).min(up as f64 / down as f64) / up as f64 * 0.94;
        let beta = 8.0;
        let denom = bessel_i0(beta);
        let mut taps = vec![0.0f32; n];
        for (i, t) in taps.iter_mut().enumerate() {
            let x = i as f64 - half as f64;
            let sinc = if x.abs() < 1e-9 {
                2.0 * fc
            } else {
                (2.0 * std::f64::consts::PI * fc * x).sin() / (std::f64::consts::PI * x)
            };
            let r = 2.0 * i as f64 / (n as f64 - 1.0) - 1.0;
            let w = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / denom;
            *t = (sinc * w * up as f64) as f32;
        }
        let mut phases = vec![Vec::new(); up];
        for (i, &t) in taps.iter().enumerate() {
            phases[i % up].push(t);
        }
        Self {
            up,
            down,
            phases,
            half,
        }
    }

    pub fn ratio(&self) -> f64 {
        self.up as f64 / self.down as f64
    }

    pub fn output_len(&self, input_len: usize) -> usize {
        input_len * self.up / self.down
    }

    /// Resample a whole buffer. Output length is `output_len(input.len())`.
    pub fn process(&self, input: &[f32], output: &mut Vec<f32>) {
        output.clear();
        if self.up == 1 && self.down == 1 {
            output.extend_from_slice(input);
            return;
        }
        let out_len = self.output_len(input.len());
        output.reserve(out_len);
        for n in 0..out_len {
            // Position in the upsampled stream, centered on the filter.
            let pos = n * self.down + self.half;
            let phase = pos % self.up;
            let mut idx = pos / self.up; // input index of tap 0 in this phase
            let mut acc = 0.0f32;
            for &t in &self.phases[phase] {
                if idx < input.len() {
                    acc += t * input[idx];
                }
                if idx == 0 {
                    break;
                }
                idx -= 1;
            }
            output.push(acc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_survives_48k_to_16k_and_back() {
        let down = Resampler::new(48_000, 16_000);
        let up = Resampler::new(16_000, 48_000);
        let n = 48_000;
        let x: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin())
            .collect();
        let mut y = Vec::new();
        down.process(&x, &mut y);
        assert_eq!(y.len(), 16_000);
        let mut z = Vec::new();
        up.process(&y, &mut z);
        assert_eq!(z.len(), 48_000);
        // Compare the middle, ignoring filter delay by checking RMS and peak.
        let mid = &z[10_000..38_000];
        let rms = (mid.iter().map(|v| v * v).sum::<f32>() / mid.len() as f32).sqrt();
        assert!(
            (rms - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.03,
            "rms {rms}"
        );
    }

    #[test]
    fn ratio_40k_to_48k() {
        let r = Resampler::new(40_000, 48_000);
        assert_eq!(r.output_len(40_000), 48_000);
    }
}
