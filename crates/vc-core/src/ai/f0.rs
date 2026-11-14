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

/// Median over five frames inside voiced runs, to tame octave flicker
/// between frames. Frames next to an unvoiced one are left as they are.
pub fn median5(f0: &mut [f32], scratch: &mut Vec<f32>) {
    if f0.len() < 5 {
        return;
    }
    scratch.clear();
    scratch.extend_from_slice(f0);
    for i in 2..scratch.len() - 2 {
        let mut v = [
            scratch[i - 2],
            scratch[i - 1],
            scratch[i],
            scratch[i + 1],
            scratch[i + 2],
        ];
        if v.iter().all(|x| *x > 0.0) {
            v.sort_by(|x, y| x.total_cmp(y));
            f0[i] = v[2];
        }
    }
}

/// Unvoice voiced runs shorter than `min_run` frames: a cough, a click or a
/// plosive that the pitch model heard a note in for a frame or two. Runs
/// that touch either end are kept, they may continue outside what we see.
pub fn despeckle(f0: &mut [f32], min_run: usize) {
    let n = f0.len();
    let mut i = 0;
    while i < n {
        if f0[i] <= 0.0 {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && f0[i] > 0.0 {
            i += 1;
        }
        if start > 0 && i < n && i - start < min_run {
            f0[start..i].fill(0.0);
        }
    }
}

/// Bring pitch outside `lo..=hi` back by whole octaves when that lands
/// inside (an octave error of the pitch model), and unvoice it otherwise
/// (not a pitch this speaker produces).
pub fn fold_into(f0: &mut [f32], lo: f32, hi: f32) {
    for v in f0.iter_mut() {
        if *v <= 0.0 || (*v >= lo && *v <= hi) {
            continue;
        }
        let mut f = *v;
        while f > hi {
            f *= 0.5;
        }
        while f < lo {
            f *= 2.0;
        }
        *v = if f <= hi { f } else { 0.0 };
    }
}

/// Median of the voiced frames, 0 when there are none.
pub fn voiced_median(f0: &[f32], scratch: &mut Vec<f32>) -> f32 {
    scratch.clear();
    scratch.extend(f0.iter().copied().filter(|v| *v > 0.0));
    if scratch.is_empty() {
        return 0.0;
    }
    scratch.sort_by(|a, b| a.total_cmp(b));
    scratch[scratch.len() / 2]
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

    #[test]
    fn despeckle_removes_short_inner_runs_only() {
        let mut f = vec![0.0, 200.0, 0.0, 0.0, 150.0, 150.0, 150.0, 0.0, 300.0];
        despeckle(&mut f, 3);
        assert_eq!(f, vec![0.0, 0.0, 0.0, 0.0, 150.0, 150.0, 150.0, 0.0, 300.0]);
        // A run at the very start may be the end of a longer one: kept.
        let mut g = vec![120.0, 0.0, 0.0, 0.0];
        despeckle(&mut g, 3);
        assert_eq!(g[0], 120.0);
    }

    #[test]
    fn fold_fixes_octave_errors_and_drops_the_impossible() {
        let mut f = vec![0.0, 120.0, 480.0, 55.0, 1500.0];
        fold_into(&mut f, 80.0, 300.0);
        assert_eq!(f[0], 0.0);
        assert_eq!(f[1], 120.0);
        assert_eq!(f[2], 240.0);
        assert_eq!(f[3], 110.0);
        assert_eq!(f[4], 187.5);
        // A range narrower than an octave cannot always be reached.
        let mut g = vec![400.0];
        fold_into(&mut g, 210.0, 300.0);
        assert_eq!(g[0], 0.0);
    }

    #[test]
    fn median5_smooths_a_spike_inside_a_voiced_run() {
        let mut f = vec![100.0, 101.0, 300.0, 102.0, 103.0, 104.0, 0.0];
        let mut scratch = Vec::new();
        median5(&mut f, &mut scratch);
        assert_eq!(f[2], 102.0);
        assert_eq!(f[5], 104.0);
        assert_eq!(f[3], 103.0);
        assert_eq!(voiced_median(&f, &mut scratch), 103.0);
    }
}
