//! Makes the converted voice as loud, moment by moment, as what went in.
//!
//! A voice model renders everything at speech level: a breath, a quiet
//! mouth noise or the tail of a word that the gate has already faded comes
//! out as loud as a shout. Following the input's loudness envelope puts
//! quiet things back where they were (RVC calls this the volume envelope).

/// Envelope resolution: 10 ms at 48 kHz.
const HOP: usize = 480;
/// Never boost by more than 6 dB (this is for putting quiet things back,
/// not for making the model louder) or cut by more than 40 dB.
const MAX_GAIN: f32 = 2.0;
const MIN_GAIN: f32 = 0.01;
/// Level below which the output is treated as silence (no boosting noise).
const FLOOR: f32 = 1e-4;

/// Scale `output` so its loudness envelope moves toward `input`'s.
/// Both cover the same stretch of time and have the same length.
/// `amount` 0 leaves the output alone, 1 follows the input completely.
/// `gains` is scratch space.
pub fn follow(input: &[f32], output: &mut [f32], amount: f32, gains: &mut Vec<f32>) {
    let n = input.len().min(output.len());
    let amount = amount.clamp(0.0, 1.0);
    if amount <= 0.0 || n < HOP {
        return;
    }
    let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
    gains.clear();
    let mut start = 0;
    while start < n {
        let end = (start + HOP).min(n);
        // A window twice the hop, centred, so neighbours overlap.
        let lo = start.saturating_sub(HOP / 2);
        let hi = (end + HOP / 2).min(n);
        let (level_in, level_out) = (rms(&input[lo..hi]), rms(&output[lo..hi]));
        let gain = if level_out < FLOOR {
            1.0
        } else {
            (level_in / level_out)
                .clamp(MIN_GAIN, MAX_GAIN)
                .powf(amount)
        };
        gains.push(gain);
        start = end;
    }
    // Interpolate between hop centres so the gain never steps.
    for (i, o) in output[..n].iter_mut().enumerate() {
        let pos = (i as f32 - HOP as f32 / 2.0) / HOP as f32;
        let a = pos.floor().max(0.0) as usize;
        let b = (a + 1).min(gains.len() - 1);
        let frac = (pos - a as f32).clamp(0.0, 1.0);
        let a = a.min(gains.len() - 1);
        *o *= gains[a] + (gains[b] - gains[a]) * frac;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(amp: impl Fn(usize) -> f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amp(i) * (i as f32 * 2.0 * std::f32::consts::PI * 200.0 / 48_000.0).sin())
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn quiet_input_makes_the_output_quiet() {
        let n = 48_000;
        // Loud first half, 30 dB quieter second half; the "model" output is
        // loud throughout.
        let input = tone(|i| if i < n / 2 { 0.3 } else { 0.01 }, n);
        let mut output = tone(|_| 0.3, n);
        let mut scratch = Vec::new();
        follow(&input, &mut output, 1.0, &mut scratch);
        let loud = rms(&output[..n / 2 - 3 * HOP]);
        let quiet = rms(&output[n / 2 + 3 * HOP..]);
        assert!((loud / rms(&input[..n / 2 - 3 * HOP]) - 1.0).abs() < 0.05);
        assert!(
            (quiet / rms(&input[n / 2 + 3 * HOP..]) - 1.0).abs() < 0.1,
            "{quiet}"
        );
    }

    #[test]
    fn zero_amount_changes_nothing_and_half_goes_halfway_in_db() {
        let n = 4800;
        let input = tone(|_| 0.03, n);
        let mut same = tone(|_| 0.3, n);
        let mut scratch = Vec::new();
        follow(&input, &mut same, 0.0, &mut scratch);
        assert!((rms(&same) - rms(&tone(|_| 0.3, n))).abs() < 1e-6);
        let mut half = tone(|_| 0.3, n);
        follow(&input, &mut half, 0.5, &mut scratch);
        // 20 dB apart, half way is 10 dB down: 0.3 / sqrt(10).
        let want = 0.3 / 10f32.sqrt() / 2f32.sqrt();
        assert!((rms(&half) / want - 1.0).abs() < 0.05, "{}", rms(&half));
    }

    #[test]
    fn silence_in_the_output_is_not_boosted() {
        let n = 4800;
        let input = tone(|_| 0.3, n);
        let mut output = vec![1e-6; n];
        let mut scratch = Vec::new();
        follow(&input, &mut output, 1.0, &mut scratch);
        assert!(output.iter().all(|v| v.abs() < 1e-5));
    }
}
