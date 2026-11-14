//! Looks at a voice model the way a careful user would after downloading
//! it: does it produce clean audio at all, does it follow pitch, does it
//! have an index? Found by converting a short synthetic phrase offline.
//!
//! What this deliberately does not do is guess the pitch a voice was
//! trained at. Scoring how well a model tracks pitch at different shifts
//! was tried for that and does not tell voices apart (a male and a female
//! voice both track well everywhere), so no pitch suggestion is made.

use super::resample::Resampler;
use super::rvc::{ModelPaths, Rvc};
use anyhow::{Result, anyhow};
use std::path::Path;

/// What the check found.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    /// "good", "fair" or "poor".
    pub grade: &'static str,
    /// Plain sentences, most important first. Empty when nothing stood out.
    pub notes: Vec<String>,
}

/// Pitch shifts the phrase is converted at, in semitones.
const SHIFTS: [f32; 3] = [-12.0, 0.0, 12.0];

/// A phrase of synthetic speech at 16 kHz: a vowel-like tone whose pitch
/// and loudness move the way a sentence does, with pauses.
pub fn test_phrase() -> Vec<f32> {
    let rate = 16_000.0f32;
    let n = (rate * 3.0) as usize;
    let formants = [(600.0f32, 1.0f32), (1400.0, 0.6), (2600.0, 0.3)];
    let mut noise = 0x9E37_79B9_7F4A_7C15u64;
    let mut phase = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / rate;
            // A falling sentence melody around 130 Hz with a little vibrato.
            let f0 =
                130.0 * (1.0 + 0.18 * (t * 2.1).sin()) * (1.0 - 0.04 * t) + 2.0 * (t * 31.0).sin();
            phase += f0 / rate;
            let mut s = 0.0f32;
            for h in 1..=30 {
                let f = f0 * h as f32;
                if f > 7000.0 {
                    break;
                }
                let mut amp = 0.6 / h as f32;
                for (fc, g) in formants {
                    let d = (f - fc) / 200.0;
                    amp += g * (-d * d).exp();
                }
                s += amp * (2.0 * std::f32::consts::PI * phase * h as f32).sin();
            }
            noise ^= noise << 13;
            noise ^= noise >> 7;
            noise ^= noise << 17;
            let hiss = ((noise >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.01;
            // Syllables: about four a second, with a pause every second.
            let syllable = (0.5 + 0.5 * (t * 4.0 * std::f32::consts::TAU).sin()).powf(0.6);
            let pause = if (t % 1.0) > 0.85 { 0.0 } else { 1.0 };
            (s * 0.05 * syllable + hiss) * pause
        })
        .collect()
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// How well the output's pitch follows what was asked for: the share of
/// frames voiced in the input that come out voiced and within a semitone
/// and a half of the requested pitch.
fn tracking(asked: &[f32], got: &[f32]) -> f32 {
    let mut voiced = 0usize;
    let mut hit = 0usize;
    for (a, g) in asked.iter().zip(got) {
        if *a <= 0.0 {
            continue;
        }
        voiced += 1;
        if *g > 0.0 && (g / a).log2().abs() * 12.0 < 1.5 {
            hit += 1;
        }
    }
    hit as f32 / voiced.max(1) as f32
}

/// Convert `phrase16k` with every shift in [`SHIFTS`] and report.
pub fn check_voice(voice: &Path, phrase16k: &[f32], threads: usize) -> Result<Report> {
    let dir = super::models_dir().ok_or_else(|| anyhow!("no data directory"))?;
    let (contentvec, rmvpe) = super::find_base_models(&dir)
        .ok_or_else(|| anyhow!("the base models are not downloaded yet"))?;
    let mut rvc = Rvc::load(
        &ModelPaths {
            contentvec,
            rmvpe,
            voice: voice.to_path_buf(),
        },
        threads.max(1),
    )?;
    let report = check_loaded(&mut rvc, voice, phrase16k);
    // GPU builds must not tear sessions down (see `stream::release`).
    #[cfg(feature = "gpu")]
    std::mem::forget(rvc);
    report
}

fn check_loaded(rvc: &mut Rvc, voice: &Path, phrase16k: &[f32]) -> Result<Report> {
    let input_f0 = rvc.pitch_of(phrase16k)?;
    let input_level = rms(phrase16k);

    let mut notes = Vec::new();
    let mut penalty = 0u32;
    let mut out = Vec::new();
    let mut out16 = Vec::new();
    let mut scores: Vec<(f32, f32)> = Vec::new();
    let mut resampler: Option<Resampler> = None;
    let mut worst_clip = 0.0f32;
    let mut level_at_zero = 0.0f32;
    for shift in SHIFTS {
        rvc.infer(phrase16k, shift, 0, &mut out)?;
        if out.iter().any(|v| !v.is_finite()) {
            return Ok(Report {
                grade: "poor",
                notes: vec!["The model produces invalid audio (not a number).".into()],
            });
        }
        let rate = rvc.model_rate().unwrap_or(40_000);
        let r = resampler.get_or_insert_with(|| Resampler::new(rate, 16_000));
        r.process(&out, &mut out16);
        let clipped = out.iter().filter(|v| v.abs() >= 0.999).count() as f32 / out.len() as f32;
        worst_clip = worst_clip.max(clipped);
        if shift == 0.0 {
            level_at_zero = rms(&out);
        }
        let got = rvc.pitch_of(&out16)?;
        let k = (shift / 12.0).exp2();
        let asked: Vec<f32> = input_f0.iter().map(|f| f * k).collect();
        scores.push((shift, tracking(&asked, &got)));
    }

    if worst_clip > 0.002 {
        penalty += 1;
        notes.push(format!(
            "The output clips ({:.1}% of samples at full scale); it will sound harsh when loud.",
            worst_clip * 100.0
        ));
    }
    if level_at_zero < input_level * 0.05 {
        penalty += 2;
        notes.push("The output is almost silent.".into());
    }
    let best = scores
        .iter()
        .copied()
        .fold((0.0f32, -1.0f32), |a, b| if b.1 > a.1 { b } else { a });
    if best.1 < 0.5 {
        penalty += 2;
        notes.push(format!(
            "It follows pitch poorly at every shift tried (best {:.0}% of frames); expect a robotic sound.",
            best.1 * 100.0
        ));
    } else if best.1 < 0.8 {
        penalty += 1;
        notes.push(format!(
            "Pitch tracking is mediocre (best {:.0}% of frames).",
            best.1 * 100.0
        ));
    }
    if !voice.with_extension("index").is_file() {
        penalty += 1;
        notes.push("No index file: it will sound less like the target and more like you.".into());
    }
    if let Some(epochs) = epochs_in_name(voice)
        && epochs < 100
    {
        notes.push(format!(
            "Trained for only {epochs} epochs according to its name; such models are often rough."
        ));
    }

    Ok(Report {
        grade: match penalty {
            0 => "good",
            1 | 2 => "fair",
            _ => "poor",
        },
        notes,
    })
}

/// An epoch count in the file name ("voice_300e_...", "... 300 Epochs").
pub fn epochs_in_name(voice: &Path) -> Option<u32> {
    let name = voice.file_stem()?.to_string_lossy().to_ascii_lowercase();
    let bytes = name.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let rest = name[i..].trim_start_matches([' ', '_', '-']);
            let boundary = |s: &str| !s.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
            let is_epochs = rest.starts_with("epoch")
                || (name[i..].starts_with('e') && boundary(&name[i + 1..]));
            if is_epochs && (start == 0 || !bytes[start - 1].is_ascii_alphanumeric()) {
                return name[start..i].parse().ok();
            }
        } else {
            i += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_epochs_in_names() {
        let e = |n: &str| epochs_in_name(Path::new(n));
        assert_eq!(e("hwyt-voice-hq_35e_7735s.onnx"), Some(35));
        assert_eq!(e("Luigi 300 Epochs.onnx"), Some(300));
        assert_eq!(e("voice_v2_40k.onnx"), None);
        assert_eq!(e("model2e5.onnx"), None);
        assert_eq!(e("MJ-Thriller-1980.onnx"), None);
    }

    #[test]
    fn tracking_counts_frames_within_a_semitone_and_a_half() {
        let asked = [0.0, 100.0, 100.0, 100.0, 100.0];
        let got = [50.0, 101.0, 0.0, 200.0, 95.0];
        assert!((tracking(&asked, &got) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn the_test_phrase_is_audible_and_bounded() {
        let p = test_phrase();
        assert_eq!(p.len(), 48_000);
        assert!(p.iter().all(|v| v.abs() < 1.0));
        assert!(rms(&p) > 0.01);
    }
}
