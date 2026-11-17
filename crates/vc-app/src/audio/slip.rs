//! Clock-drift compensation for a stream that plays on another device's
//! clock ("Hear myself": the microphone and the headphones each have their
//! own crystal, a few dozen parts per million apart).
//!
//! The ring between the two slowly fills or drains. Rather than letting it
//! run dry (a gap) or overflow (a skip), the reader takes one sample more or
//! one sample fewer than it plays in a block and stretches them to fit: a
//! 0.4% speed change for that block, far below what an ear can notice, and
//! no discontinuity. Realtime-safe.

/// Stretches a block by linear interpolation, carrying the last input
/// sample over so consecutive blocks join seamlessly.
#[derive(Default)]
pub struct Slip {
    prev: f32,
}

impl Slip {
    /// How many samples to take from a ring holding `available` to play `n`,
    /// steering the fill level toward `target` with a dead band of `band`.
    pub fn take(available: usize, n: usize, target: usize, band: usize) -> usize {
        if n < 2 {
            n
        } else if available > target + band && available > n {
            n + 1
        } else if available + band < target {
            n - 1
        } else {
            n
        }
    }

    /// Resample `src` (any length ≥ 1) onto `out`. The last output equals the
    /// last input, so the next call continues from exactly there.
    pub fn stretch(&mut self, src: &[f32], out: &mut [f32]) {
        let (m, n) = (src.len(), out.len());
        if m == 0 || n == 0 {
            return;
        }
        if m == n {
            out.copy_from_slice(src);
        } else {
            let step = m as f32 / n as f32;
            for (k, o) in out.iter_mut().enumerate() {
                // Position in a sequence whose index -1 is the carried sample.
                let pos = (k + 1) as f32 * step - 1.0;
                let base = pos.floor();
                let frac = pos - base;
                let i = base as isize;
                let a = if i < 0 { self.prev } else { src[i as usize] };
                let b = src[((i + 1).max(0) as usize).min(m - 1)];
                *o = a + (b - a) * frac;
            }
        }
        self.prev = src[m - 1];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_steers_toward_the_target() {
        assert_eq!(Slip::take(1000, 256, 512, 128), 257);
        assert_eq!(Slip::take(512, 256, 512, 128), 256);
        assert_eq!(Slip::take(600, 256, 512, 128), 256);
        assert_eq!(Slip::take(300, 256, 512, 128), 255);
    }

    #[test]
    fn stretched_blocks_join_without_a_jump() {
        const N: usize = 256;
        let tone = |i: usize| (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / 48_000.0).sin();
        // The largest step a 440 Hz sine makes between two samples.
        let natural = 2.0 * std::f32::consts::PI * 440.0 / 48_000.0;
        for m in [N - 1, N, N + 1] {
            let mut slip = Slip::default();
            let mut out = vec![0.0f32; N * 20];
            let mut pos = 0;
            for block in out.chunks_mut(N) {
                let src: Vec<f32> = (pos..pos + m).map(tone).collect();
                pos += m;
                slip.stretch(&src, block);
            }
            let worst = out[N..]
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0f32, f32::max);
            assert!(worst < natural * 1.05, "m={m}: step {worst} vs {natural}");
            // Still a full-level tone.
            let peak = out[N..].iter().fold(0.0f32, |a, b| a.max(b.abs()));
            assert!(peak > 0.99, "m={m}: peak {peak}");
        }
    }

    #[test]
    fn equal_lengths_copy_exactly() {
        let mut slip = Slip::default();
        let src = [0.1, -0.2, 0.3];
        let mut out = [0.0; 3];
        slip.stretch(&src, &mut out);
        assert_eq!(out, src);
    }
}
