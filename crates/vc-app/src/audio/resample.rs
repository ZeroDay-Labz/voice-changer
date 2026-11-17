//! Streaming sample-rate conversion for devices that do not run at the
//! engine's rate: a Kaiser-windowed sinc, interpolated between table rows,
//! with exact fixed-point stepping so the push side (microphone) and the
//! pull side (output device) always agree on how many samples they owe.
//!
//! Realtime safe: nothing is allocated after `new`.

/// Taps on each side of the kernel centre.
const HALF: usize = 16;
const TAPS: usize = 2 * HALF;
/// Table rows between two input samples.
const PHASES: usize = 256;
const FRAC_BITS: u32 = 32;
const FRAC_ONE: u64 = 1 << FRAC_BITS;

pub struct StreamResampler {
    /// Input samples per output sample, 32.32 fixed point.
    step: u64,
    /// `(PHASES + 1) * TAPS` kernel values.
    table: Vec<f32>,
    /// The tail of what was fed before, then the new input.
    buf: Vec<f32>,
    len: usize,
    /// Where the next output sample sits in `buf`, 32.32 fixed point.
    pos: u64,
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

impl StreamResampler {
    /// `max_input` is the longest block `push` or `pull` will ever be given.
    pub fn new(from_rate: u32, to_rate: u32, max_input: usize) -> Self {
        let ratio = to_rate as f64 / from_rate as f64;
        // Going down, the kernel is also the anti-alias filter.
        let cutoff = ratio.min(1.0) * 0.94;
        let beta = 8.0;
        let i0_beta = bessel_i0(beta);
        let mut table = vec![0.0f32; (PHASES + 1) * TAPS];
        for ph in 0..=PHASES {
            let frac = ph as f64 / PHASES as f64;
            let row = &mut table[ph * TAPS..(ph + 1) * TAPS];
            let mut sum = 0.0f64;
            let mut values = [0.0f64; TAPS];
            for (j, v) in values.iter_mut().enumerate() {
                let t = j as f64 - (HALF as f64 - 1.0) - frac;
                let x = std::f64::consts::PI * cutoff * t;
                let sinc = if x.abs() < 1e-9 { 1.0 } else { x.sin() / x };
                let w = (t / HALF as f64).abs();
                let window = if w >= 1.0 {
                    0.0
                } else {
                    bessel_i0(beta * (1.0 - w * w).sqrt()) / i0_beta
                };
                *v = cutoff * sinc * window;
                sum += *v;
            }
            // Unity gain at DC for every phase, so a steady level does not ripple.
            for (dst, v) in row.iter_mut().zip(values) {
                *dst = (v / sum) as f32;
            }
        }
        Self {
            step: ((from_rate as f64 / to_rate as f64) * FRAC_ONE as f64).round() as u64,
            table,
            buf: vec![0.0; TAPS + max_input + 1],
            // Half a kernel of silence in front: the first output sits at the
            // first input sample.
            len: HALF,
            pos: ((HALF - 1) as u64) << FRAC_BITS,
        }
    }

    #[inline]
    fn sample_at(&self, pos: u64) -> f32 {
        let i0 = (pos >> FRAC_BITS) as usize;
        let frac = (pos & (FRAC_ONE - 1)) as f64 / FRAC_ONE as f64 * PHASES as f64;
        let ph = frac as usize;
        let blend = (frac - ph as f64) as f32;
        let a = &self.table[ph * TAPS..(ph + 1) * TAPS];
        let b = &self.table[(ph + 1) * TAPS..(ph + 2) * TAPS];
        let x = &self.buf[i0 + 1 - HALF..i0 + 1 + HALF];
        let mut acc = 0.0f32;
        for k in 0..TAPS {
            acc += x[k] * (a[k] + (b[k] - a[k]) * blend);
        }
        acc
    }

    fn append(&mut self, input: &[f32]) {
        let room = self.buf.len() - self.len;
        let take = input.len().min(room);
        self.buf[self.len..self.len + take].copy_from_slice(&input[..take]);
        self.len += take;
    }

    /// Drop what no later output needs.
    fn compact(&mut self) {
        let first = ((self.pos >> FRAC_BITS) as usize + 1).saturating_sub(HALF);
        let first = first.min(self.len);
        if first > 0 {
            self.buf.copy_within(first..self.len, 0);
            self.len -= first;
            self.pos -= (first as u64) << FRAC_BITS;
        }
    }

    /// Feed `input`, write as many output samples as it yields (at most
    /// `out.len()`), return how many.
    pub fn push(&mut self, input: &[f32], out: &mut [f32]) -> usize {
        self.append(input);
        let mut n = 0;
        while n < out.len() && (self.pos >> FRAC_BITS) as usize + HALF < self.len {
            out[n] = self.sample_at(self.pos);
            self.pos += self.step;
            n += 1;
        }
        self.compact();
        n
    }

    /// How many input samples `pull` needs to produce `outputs` samples.
    pub fn input_needed(&self, outputs: usize) -> usize {
        if outputs == 0 {
            return 0;
        }
        let last = self.pos + (outputs as u64 - 1) * self.step;
        ((last >> FRAC_BITS) as usize + HALF + 1).saturating_sub(self.len)
    }

    /// Produce exactly `out.len()` samples from `input`, which must hold
    /// `input_needed(out.len())` samples.
    pub fn pull(&mut self, input: &[f32], out: &mut [f32]) {
        self.append(input);
        for o in out.iter_mut() {
            *o = if (self.pos >> FRAC_BITS) as usize + HALF < self.len {
                self.sample_at(self.pos)
            } else {
                0.0
            };
            self.pos += self.step;
        }
        self.compact();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, hz: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin() * 0.5)
            .collect()
    }

    /// RMS and the dominant frequency (from zero crossings) of the settled part.
    fn measure(x: &[f32], rate: u32) -> (f32, f32) {
        let x = &x[200..];
        let rms = (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt();
        let crossings = x.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        (rms, crossings as f32 * rate as f32 / x.len() as f32)
    }

    #[test]
    fn push_keeps_pitch_and_level_in_uneven_blocks() {
        for (from, to) in [
            (44_100, 48_000),
            (48_000, 44_100),
            (96_000, 48_000),
            (16_000, 48_000),
        ] {
            let input = sine(from, 1000.0, from as usize);
            let mut r = StreamResampler::new(from, to, 1024);
            let mut out = Vec::new();
            let mut scratch = vec![0.0; 8192];
            let mut at = 0;
            let mut size = 1;
            while at < input.len() {
                let end = (at + size).min(input.len());
                let n = r.push(&input[at..end], &mut scratch);
                out.extend_from_slice(&scratch[..n]);
                at = end;
                size = size % 733 + 97;
            }
            let expected = to as usize;
            // Short by the kernel's look-ahead (16 input samples) at most.
            assert!(
                out.len().abs_diff(expected) <= 60,
                "{from}->{to}: {}",
                out.len()
            );
            let (rms, hz) = measure(&out, to);
            assert!((rms - 0.3536).abs() < 0.01, "{from}->{to}: rms {rms}");
            assert!((hz - 1000.0).abs() < 3.0, "{from}->{to}: {hz} Hz");
        }
    }

    #[test]
    fn pull_asks_for_exactly_what_it_uses() {
        for (from, to) in [(48_000, 44_100), (48_000, 96_000), (48_000, 16_000)] {
            let input = sine(from, 440.0, from as usize * 2);
            let mut r = StreamResampler::new(from, to, 4096);
            let mut out = Vec::new();
            let mut at = 0;
            let mut block = vec![0.0; 480];
            for round in 0..(to as usize / 480) {
                let want = if round % 3 == 0 { 441 } else { 480 };
                let need = r.input_needed(want);
                r.pull(&input[at..at + need], &mut block[..want]);
                at += need;
                out.extend_from_slice(&block[..want]);
            }
            let ratio = at as f64 / out.len() as f64;
            assert!(
                (ratio - from as f64 / to as f64).abs() < 0.002,
                "{from}->{to}: {ratio}"
            );
            let (rms, hz) = measure(&out, to);
            assert!((rms - 0.3536).abs() < 0.01, "{from}->{to}: rms {rms}");
            assert!((hz - 440.0).abs() < 3.0, "{from}->{to}: {hz} Hz");
        }
    }

    #[test]
    fn frequencies_above_the_new_nyquist_are_removed() {
        // 30 kHz at 96 kHz cannot exist at 48 kHz: it must not fold back.
        let input = sine(96_000, 30_000.0, 96_000);
        let mut r = StreamResampler::new(96_000, 48_000, 96_000);
        let mut out = vec![0.0; 50_000];
        let n = r.push(&input, &mut out);
        let (rms, _) = measure(&out[..n], 48_000);
        assert!(rms < 0.005, "alias level {rms}");
    }
}
