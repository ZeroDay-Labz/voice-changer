//! Click hunt: pushes speech-like audio (hard onsets, pitch glides, noise
//! consonants, pauses) through the full pipeline under several parameter
//! sets and reports click-like discontinuities per case.
//! `cargo run --release -p vc-core --features ai --example click_hunt [voice.onnx]`
use nice_plug::prelude::*;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use vc_core::{Pipeline, VcParams};

const RATE: usize = 48_000;
const BLOCK: usize = 128;

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
}

/// Speech-like test signal: words with abrupt edges, glides, consonant
/// noise bursts, pauses with faint room noise.
fn speech_like(seconds: f32) -> Vec<f32> {
    let n = (RATE as f32 * seconds) as usize;
    let mut out = vec![0.0f32; n];
    let mut rng = Lcg(42);
    // room noise floor ~ -55 dBFS
    for v in out.iter_mut() {
        *v = rng.next() * 0.0018;
    }
    let mut t = 0.3f32;
    let mut word = 0;
    while t + 0.6 < seconds {
        let len = 0.15 + 0.25 * ((word * 7 % 10) as f32 / 10.0);
        let f0_a = 110.0 * (1.0 + 0.8 * ((word * 3 % 7) as f32 / 7.0));
        let f0_b = f0_a * (2.0f32).powf(((word % 5) as f32 - 2.0) * 4.0 / 12.0);
        let start = (t * RATE as f32) as usize;
        let end = ((t + len) * RATE as f32) as usize;
        // consonant burst before the word
        let cs = start.saturating_sub((0.05 * RATE as f32) as usize);
        let mut lp = 0.0f32;
        for v in &mut out[cs..start] {
            let w = rng.next();
            lp = 0.6 * lp + 0.4 * w; // crude band-ish noise
            *v += (w - lp) * 0.25;
        }
        let mut phase = 0.0f32;
        for (k, v) in out[start..end.min(n)].iter_mut().enumerate() {
            let i = start + k;
            let frac = (i - start) as f32 / (end - start) as f32;
            let f0 = f0_a + (f0_b - f0_a) * frac;
            phase += f0 / RATE as f32;
            if phase >= 1.0 {
                phase -= 1.0;
            }
            let mut s = 0.0;
            for h in 1..=18 {
                let f = f0 * h as f32;
                let env = (-(((f - 600.0) / 220.0).powi(2))).exp()
                    + 0.5 * (-(((f - 1500.0) / 350.0).powi(2))).exp()
                    + 0.25 * (-(((f - 2600.0) / 450.0).powi(2))).exp();
                s +=
                    env * (2.0 * std::f32::consts::PI * h as f32 * phase).sin() / (h as f32).sqrt();
            }
            // hard onset/offset: 2 ms edges only
            let edge = 0.002 * RATE as f32;
            let a = ((i - start) as f32 / edge).min(1.0) * ((end - i) as f32 / edge).min(1.0);
            *v += 0.35 * s * a;
        }
        t += len + 0.2 + 0.4 * ((word * 11 % 10) as f32 / 10.0);
        word += 1;
    }
    out
}

fn set<P: Param>(param: &P, value: P::Plain) {
    let n = param.preview_normalized(value);
    unsafe {
        param.as_ptr()._internal_set_normalized_value(n);
    }
}

fn clicks(x: &[f32], skip: usize) -> (usize, f32, Vec<f32>) {
    let win = 480;
    let mut count = 0;
    let mut worst = 0.0f32;
    let mut times = Vec::new();
    let mut s = skip;
    while s + win < x.len() {
        let mut rms = 0.0f32;
        let mut mx = 0.0f32;
        for i in s..s + win - 1 {
            let d = (x[i + 1] - x[i]).abs();
            rms += d * d;
            mx = mx.max(d);
        }
        let rms = (rms / win as f32).sqrt() + 1e-6;
        if mx > 8.0 * rms && mx > 0.02 {
            count += 1;
            worst = worst.max(mx / rms);
            if times.len() < 6 {
                times.push(s as f32 / RATE as f32);
            }
        }
        s += win;
    }
    (count, worst, times)
}

fn write_wav(path: &PathBuf, audio: &[f32]) {
    let mut b = Vec::with_capacity(44 + audio.len() * 4);
    let data_len = (audio.len() * 4) as u32;
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&3u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&(RATE as u32).to_le_bytes());
    b.extend_from_slice(&(RATE as u32 * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&32u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in audio {
        b.extend_from_slice(&s.to_le_bytes());
    }
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    std::fs::write(path, b).expect("write wav");
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let voice = std::env::args().nth(1);
    let input = speech_like(12.0);
    let out_dir = PathBuf::from("target/test-audio");
    write_wav(&out_dir.join("clickhunt_input.wav"), &input);
    let (c, w, _) = clicks(&input, 0);
    println!("{:<16} clicks={:<3} worst={:.1}", "input", c, w);

    type Setup = Box<dyn Fn(&VcParams)>;
    let mut cases: Vec<(&str, Setup)> = vec![
        ("default", Box::new(|_| {})),
        ("denoise_off", Box::new(|p| set(&p.denoise, false))),
        ("voice_only_off", Box::new(|p| set(&p.voice_only, false))),
        ("auto_level_off", Box::new(|p| set(&p.auto_level, false))),
        (
            "pitch_off",
            Box::new(|p| set(&p.pitch_engine, vc_core::PitchEngine::Off)),
        ),
        (
            "cleanup_off",
            Box::new(|p| {
                set(&p.denoise, false);
                set(&p.auto_level, false);
            }),
        ),
        (
            "woman",
            Box::new(|p| {
                set(&p.pitch, 6.0);
                set(&p.formant, 2.5);
            }),
        ),
        (
            "woman_vocoder",
            Box::new(|p| {
                set(&p.pitch_engine, vc_core::PitchEngine::Balanced);
                set(&p.pitch, 6.0);
                set(&p.formant, 2.5);
            }),
        ),
    ];
    if let Some(v) = voice.clone() {
        cases.push((
            "ai",
            Box::new(move |p| {
                p.set_ai_voice(&v);
                set(&p.ai_enabled, true);
            }),
        ));
    }

    for (name, setup) in cases {
        let params = VcParams::new();
        setup(&params);
        let realtime = name == "ai";
        let mut pipeline = Pipeline::new(RATE as f32, BLOCK, params.clone());
        let mut y = input.clone();
        let start = Instant::now();
        let mut next = start;
        let period = Duration::from_secs_f64(BLOCK as f64 / RATE as f64);
        if realtime {
            // let the models load before feeding audio
            std::thread::sleep(Duration::from_secs(8));
        }
        for chunk in y.chunks_mut(BLOCK) {
            pipeline.process(chunk);
            if realtime {
                next += period;
                if let Some(d) = next.checked_duration_since(Instant::now()) {
                    std::thread::sleep(d);
                }
            }
        }
        let skip = RATE; // ignore priming / warm-up second
        let (c, w, times) = clicks(&y, skip);
        let rms = (y[skip..].iter().map(|v| v * v).sum::<f32>() / (y.len() - skip) as f32).sqrt();
        println!(
            "{:<16} clicks={:<3} worst={:<5.1} rms={:.3} latency={}ms times={:?}",
            name,
            c,
            w,
            rms,
            pipeline.latency_samples() * 1000 / RATE as u32,
            times
                .iter()
                .map(|t| (t * 100.0).round() / 100.0)
                .collect::<Vec<_>>()
        );
        write_wav(&out_dir.join(format!("clickhunt_{name}.wav")), &y);
    }
}
