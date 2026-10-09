//! RMVPE salience decoding and RVC's coarse pitch encoding.

pub const N_CLASSES: usize = 360;
const CENTS_OFFSET: f32 = 1_997.379_4;

/// Decode `[frames][360]` salience into f0 in Hz (0 = unvoiced), the way
/// RMVPE's `to_local_average_cents` + `decode` do.
pub fn decode(salience: &[f32], frames: usize, threshold: f32, out: &mut Vec<f32>) {
    out.clear();
    out.reserve(frames);
    for t in 0..frames {
        let row = &salience[t * N_CLASSES..(t + 1) * N_CLASSES];
        let (center, max) =
            row.iter().enumerate().fold(
                (0usize, f32::MIN),
                |a, (i, &v)| if v > a.1 { (i, v) } else { a },
            );
        if max <= threshold {
            out.push(0.0);
            continue;
        }
        let lo = center.saturating_sub(4);
        let hi = (center + 4).min(N_CLASSES - 1);
        let mut num = 0.0f32;
        let mut den = 0.0f32;
        for (i, &v) in row.iter().enumerate().take(hi + 1).skip(lo) {
            let cents = 20.0 * i as f32 + CENTS_OFFSET;
            num += v * cents;
            den += v;
        }
        let cents = if den > 0.0 { num / den } else { 0.0 };
        let f0 = 10.0 * (cents / 1200.0).exp2();
        out.push(if (f0 - 10.0).abs() < 1e-3 { 0.0 } else { f0 });
    }
}

/// Transpose by semitones (0 stays unvoiced).
pub fn shift(f0: &mut [f32], semitones: f32) {
    let k = (semitones / 12.0).exp2();
    for v in f0.iter_mut() {
        if *v > 0.0 {
            *v *= k;
        }
    }
}

/// RVC's 1..=255 mel-scaled coarse pitch index (0 for unvoiced frames is
/// also mapped to 1, as in the reference implementation).
pub fn coarse(f0: f32) -> i64 {
    const F0_MIN: f32 = 50.0;
    const F0_MAX: f32 = 1100.0;
    let mel = |f: f32| 1127.0 * (1.0 + f / 700.0).ln();
    let (mel_min, mel_max) = (mel(F0_MIN), mel(F0_MAX));
    let mut m = mel(f0);
    if m > 0.0 {
        m = (m - mel_min) * 254.0 / (mel_max - mel_min) + 1.0;
    }
    (m.round() as i64).clamp(1, 255)
}

/// Light smoothing of voiced runs to tame octave flicker between frames.
pub fn median3(f0: &mut [f32]) {
    if f0.len() < 3 {
        return;
    }
    let src = f0.to_vec();
    for i in 1..src.len() - 1 {
        let (a, b, c) = (src[i - 1], src[i], src[i + 1]);
        if a > 0.0 && b > 0.0 && c > 0.0 {
            let mut v = [a, b, c];
            v.sort_by(|x, y| x.partial_cmp(y).unwrap());
            f0[i] = v[1];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_peak_bin_to_expected_hz() {
        let frames = 1;
        let mut sal = vec![0.0f32; N_CLASSES];
        // A440 ≈ 1200*log2(440/10) = 6551 cents → bin (6551-1997)/20 ≈ 227.7
        sal[228] = 0.9;
        sal[227] = 0.6;
        sal[229] = 0.6;
        let mut out = Vec::new();
        decode(&sal, frames, 0.03, &mut out);
        assert!((out[0] - 440.0).abs() < 5.0, "{}", out[0]);
    }

    #[test]
    fn coarse_is_monotonic_and_bounded() {
        assert_eq!(coarse(0.0), 1);
        let a = coarse(100.0);
        let b = coarse(200.0);
        let c = coarse(2000.0);
        assert!(a < b && b <= c && c == 255);
    }
}
