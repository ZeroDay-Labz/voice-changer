//! Log-mel spectrogram matching RMVPE's front end: 16 kHz, n_fft 1024,
//! hop 160, 128 HTK-scale mel bands 30 Hz..8 kHz with Slaney area
//! normalization (librosa `mel(..., htk=True)`), centered STFT with reflect
//! padding, `ln(max(mel, 1e-5))`.

use realfft::RealFftPlanner;
use realfft::num_complex::Complex;
use std::f64::consts::PI;

pub const SAMPLE_RATE: usize = 16_000;
pub const N_FFT: usize = 1024;
pub const HOP: usize = 160;
pub const N_MELS: usize = 128;
const F_MIN: f64 = 30.0;
const F_MAX: f64 = 8000.0;

fn hz_to_mel(f: f64) -> f64 {
    // HTK scale, as RMVPE's MelSpectrogram requests (`htk=True`).
    2595.0 * (1.0 + f / 700.0).log10()
}

fn mel_to_hz(m: f64) -> f64 {
    700.0 * (10f64.powf(m / 2595.0) - 1.0)
}

pub struct MelSpectrogram {
    fft: std::sync::Arc<dyn realfft::RealToComplex<f32>>,
    window: Vec<f32>,
    /// filterbank[mel][bin]
    filters: Vec<Vec<f32>>,
    frame: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    magnitude: Vec<f32>,
}

impl MelSpectrogram {
    pub fn new() -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(N_FFT);
        // Periodic Hann, like torch.hann_window.
        let window = (0..N_FFT)
            .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / N_FFT as f64).cos())
            .map(|v| v as f32)
            .collect();

        let n_bins = N_FFT / 2 + 1;
        let fft_freqs: Vec<f64> = (0..n_bins)
            .map(|i| i as f64 * SAMPLE_RATE as f64 / N_FFT as f64)
            .collect();
        let mel_lo = hz_to_mel(F_MIN);
        let mel_hi = hz_to_mel(F_MAX);
        let mel_pts: Vec<f64> = (0..N_MELS + 2)
            .map(|i| mel_to_hz(mel_lo + (mel_hi - mel_lo) * i as f64 / (N_MELS + 1) as f64))
            .collect();
        let mut filters = vec![vec![0.0f32; n_bins]; N_MELS];
        for (m, filt) in filters.iter_mut().enumerate() {
            let (lo, center, hi) = (mel_pts[m], mel_pts[m + 1], mel_pts[m + 2]);
            let enorm = 2.0 / (hi - lo); // Slaney normalization
            for (b, &f) in fft_freqs.iter().enumerate() {
                let lower = (f - lo) / (center - lo);
                let upper = (hi - f) / (hi - center);
                let w = lower.min(upper).max(0.0);
                filt[b] = (w * enorm) as f32;
            }
        }
        Self {
            spectrum: fft.make_output_vec(),
            frame: fft.make_input_vec(),
            fft,
            window,
            filters,
            magnitude: vec![0.0; n_bins],
        }
    }

    pub fn n_frames(audio_len: usize) -> usize {
        audio_len / HOP + 1
    }

    /// Compute log-mel for `audio` (16 kHz). Output is `[N_MELS][frames]`
    /// flattened mel-major, i.e. the `[1, 128, T]` layout RMVPE wants.
    pub fn compute(&mut self, audio: &[f32], out: &mut Vec<f32>) -> usize {
        let frames = Self::n_frames(audio.len());
        out.clear();
        out.resize(N_MELS * frames, 0.0);
        let pad = N_FFT / 2;
        let len = audio.len() as isize;
        for t in 0..frames {
            let start = t as isize * HOP as isize - pad as isize;
            for i in 0..N_FFT {
                let mut idx = start + i as isize;
                // Reflect padding (torch "reflect").
                if idx < 0 {
                    idx = -idx;
                }
                if idx >= len {
                    idx = 2 * (len - 1) - idx;
                }
                let s = if len > 0 {
                    audio[idx.clamp(0, len - 1) as usize]
                } else {
                    0.0
                };
                self.frame[i] = s * self.window[i];
            }
            self.fft
                .process(&mut self.frame, &mut self.spectrum)
                .expect("fft sizes match");
            for (m, c) in self.magnitude.iter_mut().zip(self.spectrum.iter()) {
                *m = (c.re * c.re + c.im * c.im).sqrt();
            }
            for (m, filt) in self.filters.iter().enumerate() {
                let mut acc = 0.0f32;
                for (w, mag) in filt.iter().zip(self.magnitude.iter()) {
                    acc += w * mag;
                }
                out[m * frames + t] = acc.max(1e-5).ln();
            }
        }
        frames
    }
}

impl Default for MelSpectrogram {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mel_roundtrip_and_shape() {
        for f in [30.0, 500.0, 1000.0, 4000.0, 8000.0] {
            assert!((mel_to_hz(hz_to_mel(f)) - f).abs() < 1e-6);
        }
        let mut mel = MelSpectrogram::new();
        let audio: Vec<f32> = (0..16_000)
            .map(|i| (2.0 * std::f32::consts::PI * 220.0 * i as f32 / 16_000.0).sin())
            .collect();
        let mut out = Vec::new();
        let frames = mel.compute(&audio, &mut out);
        assert_eq!(frames, 101);
        assert_eq!(out.len(), 128 * 101);
        // Energy should peak in a low mel band for a 220 Hz tone.
        let t = 50;
        let (best, _) = (0..N_MELS)
            .map(|m| (m, out[m * frames + t]))
            .fold((0, f32::MIN), |a, b| if b.1 > a.1 { b } else { a });
        assert!(best < 20, "peak band {best}");
    }
}
