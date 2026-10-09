//! Pitch-synchronous overlap-add (TD-PSOLA) pitch and formant shifter.
//!
//! This is the technique behind classic hardware voice transformers: find
//! the voice's period, cut two-period Hann grains, and re-lay them at the
//! new period. Formants are shifted by resampling each grain. It keeps the
//! waveform shape of the voice intact, so speech stays natural where a
//! phase vocoder turns glassy; the trade-off is a little roughness on big
//! shifts and no time-stretching.

use std::f32::consts::PI;

const F0_MIN: f32 = 80.0;
const F0_MAX: f32 = 450.0;
/// Pitch analysis hop at the input rate.
const HOP: usize = 256;
/// YIN integration window in 2x-decimated samples (~16 ms at 48 kHz).
const YIN_WINDOW_DEC: usize = 384;
const YIN_THRESHOLD: f32 = 0.15;
const VOICED_MAX_APERIODICITY: f32 = 0.35;
/// Period used to place grains in unvoiced stretches.
const UNVOICED_PERIOD_MS: f32 = 8.0;

struct Ring {
    buf: Vec<f32>,
    mask: usize,
}

impl Ring {
    fn new(min_len: usize) -> Self {
        let len = min_len.next_power_of_two();
        Self {
            buf: vec![0.0; len],
            mask: len - 1,
        }
    }
    #[inline]
    fn at(&self, i: i64) -> f32 {
        self.buf[(i as usize) & self.mask]
    }
    #[inline]
    fn at_mut(&mut self, i: i64) -> &mut f32 {
        let m = self.mask;
        &mut self.buf[(i as usize) & m]
    }
    /// 4-point cubic interpolation at a fractional index.
    #[inline]
    fn interp(&self, pos: f64) -> f32 {
        let i = pos.floor() as i64;
        let f = (pos - i as f64) as f32;
        let (y0, y1, y2, y3) = (self.at(i - 1), self.at(i), self.at(i + 1), self.at(i + 2));
        let c0 = y1;
        let c1 = 0.5 * (y2 - y0);
        let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
        ((c3 * f + c2) * f + c1) * f + c0
    }
    fn clear(&mut self) {
        self.buf.fill(0.0);
    }
}

#[derive(Clone, Copy)]
struct PitchEstimate {
    /// Input time (samples) the estimate is centred on.
    center: i64,
    /// Period in samples (input rate).
    period: f32,
    voiced: bool,
}

pub struct Psola {
    sample_rate: f32,
    pitch_ratio: f32,
    formant_ratio: f32,
    input: Ring,
    output: Ring,
    /// Samples received so far.
    in_time: i64,
    /// Next synthesis mark (input-time units).
    next_mark: f64,
    /// Analysis mark grid, advanced by the local period. Grains are cut
    /// here so consecutive grains stay phase-coherent; at unity shift the
    /// two grids coincide and the shifter is transparent.
    analysis_mark: f64,
    /// Output samples emitted so far; output index k carries input time k - latency.
    out_time: i64,
    latency: usize,
    period_max: usize,
    // pitch tracking
    dec: Vec<f32>, // 2x decimated recent input, oldest first
    dec_len: usize,
    diff: Vec<f32>,
    estimates: [PitchEstimate; 8],
    est_head: usize,
    recent_periods: [f32; 3],
    last_period: f32,
    /// Period the analysis grid advances by from `analysis_mark`, captured
    /// when that mark was reached (the estimate table keeps updating, so
    /// re-evaluating later would desynchronize the two grids).
    analysis_period: f32,
    /// Whether the previous grain was voiced (re-seat the grid only when
    /// coming out of an unvoiced stretch).
    prev_voiced: bool,
    /// Period smoothed across analysis frames (one-pole), so both grids see
    /// the same gently varying P(t).
    period_lp: f32,
    /// Consecutive frames judged unvoiced (hysteresis before we actually switch).
    unvoiced_run: u32,
    /// Frames since the last confidently voiced estimate.
    since_voiced: u32,
}

impl Psola {
    pub fn new(sample_rate: f32) -> Self {
        let period_max = (sample_rate / F0_MIN).ceil() as usize;
        let tau_max_dec = period_max.div_ceil(2);
        let analysis_len = 2 * (YIN_WINDOW_DEC + tau_max_dec);
        // A mark t_s is synthesized once input up to t_s + P_max exists (the
        // analysis mark never sits ahead of t_s, the grain reaches P beyond
        // it); its grain starts up to 1.5 P_max before t_s in the output.
        // The pitch window centred on t_s is already available by then.
        let latency = 5 * period_max / 2 + 64;
        let unvoiced = UNVOICED_PERIOD_MS * 0.001 * sample_rate;
        Self {
            sample_rate,
            pitch_ratio: 1.0,
            formant_ratio: 1.0,
            input: Ring::new(analysis_len + 4 * period_max + 8192),
            output: Ring::new(latency + 4 * period_max + 8192),
            in_time: 0,
            next_mark: 0.0,
            analysis_mark: 0.0,
            out_time: 0,
            latency,
            period_max,
            dec: vec![0.0; YIN_WINDOW_DEC + tau_max_dec + 2],
            dec_len: YIN_WINDOW_DEC + tau_max_dec,
            diff: vec![0.0; tau_max_dec + 1],
            estimates: [PitchEstimate {
                center: 0,
                period: unvoiced,
                voiced: false,
            }; 8],
            est_head: 0,
            recent_periods: [unvoiced; 3],
            last_period: unvoiced,
            analysis_period: unvoiced,
            prev_voiced: false,
            period_lp: unvoiced,
            unvoiced_run: 0,
            since_voiced: 1000,
        }
    }

    pub fn latency_samples(&self) -> usize {
        self.latency
    }

    pub fn set_shift(&mut self, pitch_semitones: f32, formant_semitones: f32) {
        self.pitch_ratio = (pitch_semitones / 12.0).exp2();
        self.formant_ratio = (formant_semitones / 12.0).exp2().clamp(0.5, 2.0);
    }

    pub fn reset(&mut self) {
        self.input.clear();
        self.output.clear();
        self.in_time = 0;
        self.out_time = 0;
        self.next_mark = 0.0;
        self.analysis_mark = 0.0;
        let unvoiced = UNVOICED_PERIOD_MS * 0.001 * self.sample_rate;
        self.estimates.fill(PitchEstimate {
            center: 0,
            period: unvoiced,
            voiced: false,
        });
        self.recent_periods = [unvoiced; 3];
        self.last_period = unvoiced;
        self.analysis_period = unvoiced;
        self.prev_voiced = false;
        self.period_lp = unvoiced;
        self.unvoiced_run = 0;
        self.since_voiced = 1000;
        self.dec.fill(0.0);
    }

    /// Process in place. Realtime-safe.
    pub fn process(&mut self, buf: &mut [f32]) {
        for x in buf.iter_mut() {
            *self.input.at_mut(self.in_time) = *x;
            self.in_time += 1;
            if self.in_time % HOP as i64 == 0 {
                self.analyze();
            }
            self.synthesize_up_to(self.in_time - self.period_max as i64 - 2);
            // Emit the output sample for input time (out_time - latency).
            let idx = self.out_time;
            *x = if idx >= self.latency as i64 {
                let v = self.output.at(idx - self.latency as i64);
                *self.output.at_mut(idx - self.latency as i64) = 0.0;
                v
            } else {
                0.0
            };
            self.out_time += 1;
        }
    }

    /// Run YIN on the most recent analysis window (2x decimated).
    fn analyze(&mut self) {
        let dec_len = self.dec_len;
        let n_in = dec_len * 2;
        let start = self.in_time - n_in as i64;
        if start < 0 {
            return;
        }
        for (k, d) in self.dec.iter_mut().take(dec_len).enumerate() {
            let i = start + 2 * k as i64;
            *d = 0.5 * (self.input.at(i) + self.input.at(i + 1));
        }
        let center = self.in_time - (n_in / 2) as i64;

        let tau_min = ((self.sample_rate / 2.0) / F0_MAX).floor().max(2.0) as usize;
        let tau_max = self.diff.len() - 1;
        let w = YIN_WINDOW_DEC;
        // Difference function.
        let mut energy = 0.0f32;
        for j in 0..w {
            energy += self.dec[j] * self.dec[j];
        }
        if energy < 1e-7 {
            self.push_estimate(center, self.last_period, false);
            return;
        }
        self.diff[0] = 1.0;
        let mut running = 0.0f32;
        let mut best_tau = 0usize;
        let mut best_val = f32::MAX;
        let mut found_below = false;
        for tau in 1..=tau_max {
            let mut d = 0.0f32;
            let a = &self.dec[..w];
            let b = &self.dec[tau..tau + w];
            for (x, y) in a.iter().zip(b.iter()) {
                let e = x - y;
                d += e * e;
            }
            running += d;
            let cmnd = if running > 0.0 {
                d * tau as f32 / running
            } else {
                1.0
            };
            self.diff[tau] = cmnd;
            if tau >= tau_min {
                if found_below {
                    // Walk down to the local minimum after crossing the threshold.
                    if cmnd < best_val {
                        best_val = cmnd;
                        best_tau = tau;
                    } else {
                        break;
                    }
                } else if cmnd < YIN_THRESHOLD {
                    found_below = true;
                    best_val = cmnd;
                    best_tau = tau;
                } else if cmnd < best_val {
                    best_val = cmnd;
                    best_tau = tau;
                }
            }
        }
        let voiced = found_below || best_val < VOICED_MAX_APERIODICITY;
        if !voiced || best_tau < tau_min || best_tau >= tau_max {
            // Hysteresis: one doubtful frame in a voiced run keeps the last
            // period (a dropped frame mid-vowel is far more audible than a
            // late unvoiced switch).
            self.unvoiced_run += 1;
            self.since_voiced += 1;
            let still_voiced = self.unvoiced_run < 2 && self.since_voiced < 6;
            self.push_estimate(center, self.last_period, still_voiced);
            return;
        }
        self.unvoiced_run = 0;
        // Octave-jump suppression: if we were voiced a moment ago and the new
        // period is roughly half or double, prefer the lag near the old
        // period when it is also a decent minimum.
        if self.since_voiced < 6 {
            let old_tau = (self.last_period / 2.0).round() as usize;
            let ratio = best_tau as f32 / old_tau.max(1) as f32;
            if !(0.7..1.4).contains(&ratio) && old_tau > tau_min && old_tau + 1 < tau_max {
                let (mut lo, mut hi) = (
                    old_tau.saturating_sub(3).max(tau_min),
                    (old_tau + 3).min(tau_max - 1),
                );
                if lo > hi {
                    std::mem::swap(&mut lo, &mut hi);
                }
                let (near_tau, near_val) = (lo..=hi)
                    .map(|t| (t, self.diff[t]))
                    .fold((old_tau, f32::MAX), |a, b| if b.1 < a.1 { b } else { a });
                if near_val < 0.35 {
                    best_tau = near_tau;
                }
            }
        }
        self.since_voiced = 0;
        // Parabolic refinement.
        let (y0, y1, y2) = (
            self.diff[best_tau - 1],
            self.diff[best_tau],
            self.diff[best_tau + 1],
        );
        let denom = y0 - 2.0 * y1 + y2;
        let delta = if denom.abs() > 1e-9 {
            0.5 * (y0 - y2) / denom
        } else {
            0.0
        };
        let period = 2.0 * (best_tau as f32 + delta.clamp(-1.0, 1.0));
        // Median of the last three voiced periods tames octave flicker.
        self.recent_periods.rotate_left(1);
        self.recent_periods[2] = period;
        let mut sorted = self.recent_periods;
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let smoothed = sorted[1];
        // Glide between frames instead of stepping; snap when voicing (re)starts.
        if self.since_voiced == 0 && (self.period_lp - smoothed).abs() < 0.3 * smoothed {
            self.period_lp += 0.5 * (smoothed - self.period_lp);
        } else {
            self.period_lp = smoothed;
        }
        self.last_period = self.period_lp;
        self.push_estimate(center, self.period_lp, true);
    }

    fn push_estimate(&mut self, center: i64, period: f32, voiced: bool) {
        self.est_head = (self.est_head + 1) % self.estimates.len();
        self.estimates[self.est_head] = PitchEstimate {
            center,
            period,
            voiced,
        };
    }

    /// Period to use at input time `t`: the tracked period when voiced, the
    /// fixed unvoiced spacing otherwise.
    fn period_at(&self, t: f64) -> f32 {
        let est = self.estimate_at(t as i64);
        if est.voiced {
            est.period.clamp(8.0, self.period_max as f32)
        } else {
            UNVOICED_PERIOD_MS * 0.001 * self.sample_rate
        }
    }

    fn estimate_at(&self, t: i64) -> PitchEstimate {
        let mut best = self.estimates[self.est_head];
        let mut best_d = i64::MAX;
        for e in &self.estimates {
            let d = (e.center - t).abs();
            if d < best_d {
                best_d = d;
                best = *e;
            }
        }
        best
    }

    /// Place grains for every synthesis mark whose input is available.
    fn synthesize_up_to(&mut self, limit: i64) {
        let mut guard = 0;
        while (self.next_mark as i64) <= limit && guard < 64 {
            guard += 1;
            let t_s = self.next_mark;
            let est = self.estimate_at(t_s as i64);
            let period = self.period_at(t_s);
            let ratio = if est.voiced { self.pitch_ratio } else { 1.0 };
            // Analysis marks a_{j+1} = a_j + P(a_j); synthesis marks
            // t_{k+1} = t_k + P(t_k)/ratio. Pick the last analysis mark at or
            // before t_s. At unity the two sequences coincide exactly (so
            // the shifter is transparent) however P varies; at other ratios
            // marks get repeated or skipped, which is PSOLA. Re-seat only
            // when voicing starts: noise has no phase worth preserving.
            if est.voiced && !self.prev_voiced {
                self.analysis_mark = t_s;
                self.analysis_period = period;
            }
            self.prev_voiced = est.voiced;
            let mut steps = 0;
            while self.analysis_mark + (self.analysis_period as f64) <= t_s + 1e-6 && steps < 64 {
                self.analysis_mark += self.analysis_period as f64;
                self.analysis_period = self.period_at(self.analysis_mark);
                steps += 1;
            }
            steps = 0;
            while self.analysis_mark > t_s + 1e-6 && steps < 64 {
                let p = self.period_at(self.analysis_mark - 1.0) as f64;
                self.analysis_mark -= p;
                self.analysis_period = self.period_at(self.analysis_mark);
                steps += 1;
            }
            let m = self.analysis_mark;
            let f = self.formant_ratio;
            // Output grain half-length after resampling the 2P input grain by
            // f, capped so formant-down shifts don't stretch grains beyond
            // what the latency budget covers.
            let half_out = (period / f).min(1.5 * period);
            let gain = (f / ratio).clamp(0.25, 4.0) * 1.0;
            let len = (2.0 * half_out).round().max(2.0) as i64;
            let start = (t_s - half_out as f64).round() as i64;
            for n in 0..len {
                let pos_out = start + n;
                // Position within the grain in [-1, 1).
                let u = (n as f32 + 0.5) / len as f32 * 2.0 - 1.0;
                let win = 0.5 * (1.0 + (PI * u).cos());
                let src = m + (u * period) as f64;
                *self.output.at_mut(pos_out) += win * gain * self.input.interp(src);
            }
            self.next_mark += (period / ratio) as f64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    fn zero_cross_freq(x: &[f32]) -> f32 {
        let crossings = x
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        crossings as f32 / 2.0 / (x.len() as f32 / RATE)
    }

    /// A pulse train through a resonator: periodic like a voice, with harmonics.
    fn voice_like(f0: f32, seconds: f32) -> Vec<f32> {
        let n = (RATE * seconds) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / RATE;
                let mut s = 0.0;
                for h in 1..=15 {
                    let f = f0 * h as f32;
                    let env = (-(((f - 700.0) / 300.0).powi(2))).exp()
                        + 0.5 * (-(((f - 1800.0) / 400.0).powi(2))).exp();
                    s += env * (2.0 * PI * f * t).sin() / (h as f32).sqrt();
                }
                0.3 * s
            })
            .collect()
    }

    fn run(p: &mut Psola, x: &[f32]) -> Vec<f32> {
        let mut y = x.to_vec();
        for chunk in y.chunks_mut(256) {
            p.process(chunk);
        }
        y
    }

    #[test]
    fn octave_up_doubles_pitch() {
        let mut p = Psola::new(RATE);
        p.set_shift(12.0, 0.0);
        let y = run(&mut p, &voice_like(150.0, 1.5));
        let tail = &y[y.len() / 2..];
        // Count fundamental periodicity via autocorrelation peak.
        let f = dominant_period_hz(tail);
        assert!((f - 300.0).abs() < 12.0, "measured {f} Hz");
        assert!(tail.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn formant_shift_keeps_pitch() {
        let mut p = Psola::new(RATE);
        p.set_shift(0.0, 7.0);
        let y = run(&mut p, &voice_like(150.0, 1.5));
        let tail = &y[y.len() / 2..];
        let f = dominant_period_hz(tail);
        assert!((f - 150.0).abs() < 8.0, "measured {f} Hz");
    }

    /// A +6 st shift of a pitch glide with hard word edges must not click.
    #[test]
    fn glide_has_no_clicks() {
        let mut p = Psola::new(RATE);
        p.set_shift(6.0, 2.0);
        let n = (RATE * 3.0) as usize;
        let mut phase = 0.0f32;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / RATE;
                let f0 = 120.0 + 80.0 * (t / 3.0);
                phase += f0 / RATE;
                // words with 3 ms edges so the input itself has no clicks
                let pos = t % 0.7;
                let edge = 0.003;
                let env = if pos < 0.45 {
                    (pos / edge).min(1.0) * ((0.45 - pos) / edge).min(1.0)
                } else {
                    0.0
                };
                let mut s = 0.0;
                for h in 1..=12 {
                    s += (2.0 * PI * h as f32 * phase).sin() / h as f32;
                }
                0.3 * s * env
            })
            .collect();
        let y = run(&mut p, &x);
        let win = 480;
        let mut clicks = 0;
        let mut s = 4800;
        while s + win < y.len() {
            let mut rms = 0.0f32;
            let mut mx = 0.0f32;
            for i in s..s + win - 1 {
                let d = (y[i + 1] - y[i]).abs();
                rms += d * d;
                mx = mx.max(d);
            }
            let rms = (rms / win as f32).sqrt() + 1e-6;
            if mx > 8.0 * rms && mx > 0.02 {
                clicks += 1;
            }
            s += win;
        }
        assert!(y.iter().all(|v| v.is_finite()));
        assert!(clicks <= 1, "clicks: {clicks}");
    }

    #[test]
    fn unity_is_near_transparent_after_latency() {
        let mut p = Psola::new(RATE);
        let x = voice_like(150.0, 1.0);
        let y = run(&mut p, &x);
        let l = p.latency_samples();
        let mut err = 0.0f32;
        let mut ref_e = 0.0f32;
        for i in (l + 4800)..x.len() {
            let d = y[i] - x[i - l];
            err += d * d;
            ref_e += x[i - l] * x[i - l];
        }
        let snr = 10.0 * (ref_e / err.max(1e-12)).log10();
        assert!(snr > 10.0, "snr {snr} dB");
        let _ = zero_cross_freq(&y);
    }

    /// Fundamental via normalized autocorrelation: the smallest lag whose
    /// correlation is within 5% of the global maximum.
    fn dominant_period_hz(x: &[f32]) -> f32 {
        let min_lag = (RATE / 500.0) as usize;
        let max_lag = (RATE / 60.0) as usize;
        let n = x.len() - max_lag;
        let energy: f32 = x[..n].iter().map(|v| v * v).sum();
        let corr: Vec<f32> = (0..max_lag)
            .map(|lag| {
                x[..n]
                    .iter()
                    .zip(&x[lag..lag + n])
                    .map(|(a, b)| a * b)
                    .sum::<f32>()
                    / energy
            })
            .collect();
        let max = corr[min_lag..].iter().cloned().fold(f32::MIN, f32::max);
        let mut lag = min_lag;
        while lag < max_lag && corr[lag] < 0.95 * max {
            lag += 1;
        }
        // refine to the local peak
        while lag + 1 < max_lag && corr[lag + 1] > corr[lag] {
            lag += 1;
        }
        RATE / lag as f32
    }
}
