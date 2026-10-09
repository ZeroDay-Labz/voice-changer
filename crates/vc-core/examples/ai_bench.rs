//! Offline check of the AI chain: loads the models, converts a test signal
//! (or a 16-bit/float mono WAV), prints per-stage timings and writes the
//! result next to the input.
//! `cargo run --release -p vc-core --features ai --example ai_bench -- voice.onnx [input.wav] [semitones]`
use std::path::PathBuf;
use vc_core::ai::resample::Resampler;
use vc_core::ai::rvc::{ModelPaths, Rvc};

fn read_wav(path: &str) -> anyhow::Result<(Vec<f32>, usize)> {
    let b = std::fs::read(path)?;
    let fmt = b
        .windows(4)
        .position(|w| w == b"fmt ")
        .ok_or_else(|| anyhow::anyhow!("no fmt"))?;
    let format = u16::from_le_bytes([b[fmt + 8], b[fmt + 9]]);
    let channels = u16::from_le_bytes([b[fmt + 10], b[fmt + 11]]) as usize;
    let rate = u32::from_le_bytes([b[fmt + 12], b[fmt + 13], b[fmt + 14], b[fmt + 15]]) as usize;
    let bits = u16::from_le_bytes([b[fmt + 22], b[fmt + 23]]);
    let data = b
        .windows(4)
        .position(|w| w == b"data")
        .ok_or_else(|| anyhow::anyhow!("no data"))?;
    let size = u32::from_le_bytes([b[data + 4], b[data + 5], b[data + 6], b[data + 7]]) as usize;
    let pcm = &b[data + 8..(data + 8 + size).min(b.len())];
    let mut mono = Vec::new();
    match (format, bits) {
        (1, 16) => {
            for frame in pcm.chunks_exact(2 * channels) {
                let s: f32 = frame
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0)
                    .sum();
                mono.push(s / channels as f32);
            }
        }
        (3, 32) => {
            for frame in pcm.chunks_exact(4 * channels) {
                let s: f32 = frame
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| f32::from_le_bytes(*c))
                    .sum();
                mono.push(s / channels as f32);
            }
        }
        _ => anyhow::bail!("unsupported wav format {format}/{bits}"),
    }
    Ok((mono, rate))
}

fn write_wav(path: &PathBuf, audio: &[f32], rate: usize) -> anyhow::Result<()> {
    let mut b = Vec::with_capacity(44 + audio.len() * 2);
    let data_len = (audio.len() * 2) as u32;
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&(rate as u32).to_le_bytes());
    b.extend_from_slice(&(rate as u32 * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in audio {
        b.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, b)?;
    Ok(())
}

/// A vowel-ish test tone: harmonics of 140 Hz with formant-like weighting,
/// amplitude-modulated so it isn't a flat drone.
fn test_signal(rate: usize, seconds: f32) -> Vec<f32> {
    let n = (rate as f32 * seconds) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / rate as f32;
            let f0 = 140.0 * (1.0 + 0.03 * (2.0 * std::f32::consts::PI * 0.7 * t).sin());
            let mut s = 0.0;
            for h in 1..=20 {
                let f = f0 * h as f32;
                let formant = (-(((f - 650.0) / 250.0).powi(2))).exp()
                    + 0.6 * (-(((f - 1200.0) / 300.0).powi(2))).exp()
                    + 0.3 * (-(((f - 2500.0) / 400.0).powi(2))).exp();
                s += formant * (2.0 * std::f32::consts::PI * f * t).sin() / h as f32;
            }
            let env = 0.5 + 0.5 * (2.0 * std::f32::consts::PI * 2.0 * t).sin().max(0.0);
            0.4 * s * env
        })
        .collect()
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut args = std::env::args().skip(1);
    let voice =
        PathBuf::from(args.next().ok_or_else(|| {
            anyhow::anyhow!("usage: ai_bench voice.onnx [input.wav] [semitones]")
        })?);
    let input = args.next().filter(|s| !s.is_empty());
    let semitones: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let window_s: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1.13);
    let threads: usize = std::env::var("AI_THREADS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);

    let dir = vc_core::ai::models_dir().ok_or_else(|| anyhow::anyhow!("no data dir"))?;
    let (mut contentvec, mut rmvpe) = vc_core::ai::find_base_models(&dir)
        .ok_or_else(|| anyhow::anyhow!("base models missing in {}", dir.display()))?;
    if let Ok(p) = std::env::var("AI_CONTENTVEC") {
        contentvec = PathBuf::from(p);
    }
    if let Ok(p) = std::env::var("AI_RMVPE") {
        rmvpe = PathBuf::from(p);
    }
    println!(
        "content: {}  pitch: {}",
        contentvec.display(),
        rmvpe.display()
    );
    let t0 = std::time::Instant::now();
    let mut rvc = Rvc::load(
        &ModelPaths {
            contentvec,
            rmvpe,
            voice: voice.clone(),
        },
        threads,
    )?;
    println!(
        "models loaded in {:.0} ms ({threads} threads)",
        t0.elapsed().as_secs_f32() * 1000.0
    );
    let index_path = voice.with_extension("index");
    if index_path.is_file() {
        match vc_core::ai::RetrievalIndex::load(&index_path) {
            Ok(idx) => {
                println!(
                    "index: {} vectors ({:.0} MB)",
                    idx.ntotal,
                    idx.memory_bytes() as f32 / 1e6
                );
                rvc.index = Some(idx);
                rvc.index_rate = std::env::var("AI_INDEX_RATE")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0.75);
            }
            Err(e) => println!("index unusable: {e:#}"),
        }
    }
    rvc.warm_up(
        (16_000.0 * window_s) as usize,
        (16_000.0 * window_s) as usize,
    );

    let (audio, rate) = match &input {
        Some(p) => read_wav(p)?,
        None => (test_signal(48_000, window_s.max(1.2)), 48_000),
    };
    let mut audio16 = Vec::new();
    Resampler::new(rate, 16_000).process(&audio, &mut audio16);
    println!(
        "input {:.2} s at {rate} Hz → {} samples at 16 kHz",
        audio.len() as f32 / rate as f32,
        audio16.len()
    );

    // Realtime-sized window, like the streaming worker uses.
    let window: Vec<f32> =
        audio16[audio16.len().saturating_sub((16_000.0 * window_s) as usize)..].to_vec();
    let mut out = Vec::new();
    for i in 0..4 {
        let t = rvc.infer(&window, semitones, 0, &mut out)?;
        println!(
            "run {i}: window {:.2} s → features {:.0} ms, pitch {:.0} ms, synth {:.0} ms, total {:.0} ms; f0≈{:.0} Hz; out {} samples",
            window.len() as f32 / 16_000.0,
            t.features_ms,
            t.pitch_ms,
            t.synth_ms,
            t.total_ms(),
            rvc.last_f0_hz,
            out.len()
        );
    }
    let model_rate = rvc.model_rate().unwrap_or(40_000);
    let peak = out.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    let finite = out.iter().all(|x| x.is_finite());
    println!("model rate {model_rate} Hz, output peak {peak:.3}, finite {finite}");

    // Full-length conversion of the whole input for listening.
    let mut full = Vec::new();
    rvc.infer(&audio16, semitones, 0, &mut full)?;
    let out_path = input
        .as_ref()
        .map(|p| PathBuf::from(format!("{}.converted.wav", p.trim_end_matches(".wav"))))
        .unwrap_or_else(|| std::env::temp_dir().join("vc_ai_test.converted.wav"));
    write_wav(&out_path, &full, model_rate)?;
    println!("wrote {}", out_path.display());
    Ok(())
}
