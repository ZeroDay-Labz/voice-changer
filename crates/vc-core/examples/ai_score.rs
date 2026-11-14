//! Runs a recording through the realtime AI stage exactly as the app does
//! (same worker, real time) and scores the result, so a change can be
//! judged by numbers and by listening to the file it writes.
//!
//! `cargo run --release -p vc-core --features ai --example ai_score -- voice.onnx clip.wav [key=value ...]`
//!
//! Keys: `pitch=3.5` `protect=0.67` `follow=0.75` `block=on|off` `index=0.75`
//! `device=auto|cpu` `event=4.4-6.0` (seconds in the clip where a cough or
//! the like is; reports how much of it gets through) `out=file.wav`.
use nice_plug::prelude::Param;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use vc_core::VcParams;
use vc_core::ai::{AiState, AiWorker, StreamConfig, wav};

const SR: usize = 48_000;
const FRAME: usize = 480;

fn set(param: &impl Param<Plain = f32>, value: f32) {
    // The same internal setter the GUI uses.
    unsafe {
        param
            .as_ptr()
            ._internal_set_normalized_value(param.preview_normalized(value))
    };
}

fn set_bool(param: &nice_plug::prelude::BoolParam, on: bool) {
    unsafe {
        param
            .as_ptr()
            ._internal_set_normalized_value(if on { 1.0 } else { 0.0 })
    };
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

fn frame_levels(x: &[f32]) -> Vec<f32> {
    x.chunks(FRAME)
        .map(|c| db((c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt()))
        .collect()
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let mut args = std::env::args().skip(1);
    let usage = "usage: ai_score voice.onnx clip.wav [key=value ...]";
    let voice = args.next().ok_or_else(|| anyhow::anyhow!(usage))?;
    let clip = PathBuf::from(args.next().ok_or_else(|| anyhow::anyhow!(usage))?);

    let params = VcParams::new();
    params.set_ai_voice(&voice);
    set_bool(&params.ai_enabled, true);
    let mut event: Option<(f32, f32)> = None;
    let mut out_path = clip.with_extension("converted.wav");
    for arg in args {
        let (key, value) = arg
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("{arg:?} is not key=value"))?;
        match key {
            "pitch" => set(&params.ai_pitch, value.parse()?),
            "protect" => set(&params.ai_protect, value.parse()?),
            "follow" => set(&params.ai_volume_follow, value.parse()?),
            "index" => set(&params.ai_index_rate, value.parse()?),
            "block" => set_bool(&params.ai_block_noises, value == "on"),
            "device" => params.set_ai_device(value),
            "out" => out_path = PathBuf::from(value),
            "event" => {
                let (a, b) = value
                    .split_once('-')
                    .ok_or_else(|| anyhow::anyhow!("event=start-end in seconds"))?;
                event = Some((a.parse()?, b.parse()?));
            }
            _ => anyhow::bail!("unknown key {key:?}"),
        }
    }

    let (audio, rate) = wav::read_mono(&clip)?;
    anyhow::ensure!(rate == SR, "the clip must be 48 kHz (it is {rate} Hz)");
    let (mut stage, _handle) = AiWorker::spawn(params.clone(), StreamConfig::default());
    let status = stage.status();

    // Warm up on the start of the clip until the stage converts, then play
    // the clip once from the top, then a second of silence to flush.
    let quantum = 256usize;
    let period = Duration::from_secs_f64(quantum as f64 / SR as f64);
    let mut buf = vec![0.0f32; quantum];
    let mut next = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut pos = 0usize;
    loop {
        anyhow::ensure!(Instant::now() < deadline, "the AI voice never became ready");
        if status.state() == AiState::Error {
            anyhow::bail!("AI error: {}", status.message());
        }
        for x in buf.iter_mut() {
            *x = audio[pos % audio.len()];
            pos += 1;
        }
        let converted = stage.process(&mut buf);
        next += period;
        if let Some(d) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
        // Start the measured pass at a clip boundary once it has run warm
        // for two seconds.
        if converted && pos >= SR * 2 && pos % audio.len() < quantum {
            break;
        }
    }
    let blocked_before = status.blocked.load(Ordering::Relaxed);
    let dropouts_before = status.dropouts.load(Ordering::Relaxed);
    let total = audio.len() + SR * 3 / 2;
    let mut output = Vec::with_capacity(total);
    let mut fed = 0usize;
    while fed < total {
        for (i, x) in buf.iter_mut().enumerate() {
            *x = audio.get(fed + i).copied().unwrap_or(0.0);
        }
        // Whatever is not converted (a late block) is the live voice: for
        // scoring that counts as nothing coming out.
        if !stage.process(&mut buf) {
            buf.fill(0.0);
        }
        output.extend_from_slice(&buf);
        fed += quantum;
        next += period;
        if let Some(d) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
    }
    let delay = stage.latency_samples();
    // Line the output up with the input using the stage's own delay figure,
    // refined by the best match of the loudness envelopes around it.
    let in_levels = frame_levels(&audio);
    let best_shift = (delay.saturating_sub(SR / 5)..delay + SR / 5)
        .step_by(FRAME / 4)
        .min_by(|a, b| {
            let cost = |shift: usize| {
                let out_levels = frame_levels(&output[shift.min(output.len())..]);
                in_levels
                    .iter()
                    .zip(&out_levels)
                    .map(|(i, o)| (i.max(-70.0) - o.max(-70.0)).abs())
                    .sum::<f32>()
            };
            cost(*a).total_cmp(&cost(*b))
        })
        .unwrap_or(delay);
    let aligned = &output[best_shift.min(output.len())..];
    let aligned = &aligned[..aligned.len().min(audio.len())];
    let out_levels = frame_levels(aligned);

    // 1. What comes out while nothing (or next to nothing) goes in.
    let quiet: Vec<f32> = in_levels
        .iter()
        .zip(&out_levels)
        .filter(|(i, _)| **i < -50.0)
        .map(|(_, o)| *o)
        .collect();
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len().max(1) as f32;
    // 2. How well loudness is followed where there is sound.
    let pairs: Vec<(f32, f32)> = in_levels
        .iter()
        .zip(&out_levels)
        .filter(|(i, _)| **i > -50.0)
        .map(|(i, o)| (*i, o.max(-70.0)))
        .collect();
    let (mi, mo) = (
        mean(&pairs.iter().map(|p| p.0).collect::<Vec<_>>()),
        mean(&pairs.iter().map(|p| p.1).collect::<Vec<_>>()),
    );
    let (mut cov, mut vi, mut vo) = (0.0, 0.0, 0.0);
    for (i, o) in &pairs {
        cov += (i - mi) * (o - mo);
        vi += (i - mi) * (i - mi);
        vo += (o - mo) * (o - mo);
    }
    let correlation = cov / (vi * vo).sqrt().max(1e-9);
    println!(
        "clip {} ({:.1} s), voice {}",
        clip.display(),
        audio.len() as f32 / SR as f32,
        Path::new(&voice)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
    );
    println!(
        "  {} · delay {} ms (reported {} ms) · {:.0} ms/block · dropouts {}",
        status.message(),
        best_shift * 1000 / SR,
        delay * 1000 / SR,
        status.infer_ms.load(Ordering::Relaxed),
        status.dropouts.load(Ordering::Relaxed) - dropouts_before,
    );
    println!(
        "  output while the input is quiet (< -50 dB): {:.1} dB over {} frames",
        mean(&quiet),
        quiet.len()
    );
    println!(
        "  loudness: input {mi:.1} dB, output {mo:.1} dB, envelope correlation {correlation:.2}"
    );
    if let Some((a, b)) = event {
        let span = |x: &[f32]| {
            let (s, e) = ((a * SR as f32) as usize, (b * SR as f32) as usize);
            let x = &x[s.min(x.len())..e.min(x.len())];
            db((x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt())
        };
        // How long after the event starts the output is still above -45 dB.
        let start_frame = (a * 100.0) as usize;
        let end_frame = ((b * 100.0) as usize).min(out_levels.len());
        let leaked = out_levels[start_frame.min(end_frame)..end_frame]
            .iter()
            .filter(|l| **l > -45.0)
            .count();
        println!(
            "  event {a:.2}-{b:.2} s: input {:.1} dB, output {:.1} dB, audible for {} ms of {} ms · blocked sounds {}",
            span(&audio),
            span(aligned),
            leaked * 10,
            (end_frame.saturating_sub(start_frame)) * 10,
            status.blocked.load(Ordering::Relaxed) - blocked_before,
        );
    }
    // 3. Pitch steadiness of what came out: jumps of more than four
    // semitones from one 10 ms frame to the next are the robotic chirps.
    drop(_handle);
    if let Some(dir) = vc_core::ai::models_dir()
        && let Some((contentvec, rmvpe)) = vc_core::ai::find_base_models(&dir)
    {
        let mut out16 = Vec::new();
        vc_core::ai::resample::Resampler::new(SR, 16_000).process(aligned, &mut out16);
        let mut rvc = vc_core::ai::Rvc::load(
            &vc_core::ai::ModelPaths {
                contentvec,
                rmvpe,
                voice: PathBuf::from(&voice),
            },
            4,
        )?;
        let f0 = rvc.pitch_of(&out16)?;
        let voiced = f0.iter().filter(|v| **v > 0.0).count();
        let jumps = f0
            .windows(2)
            .filter(|w| w[0] > 0.0 && w[1] > 0.0 && (w[1] / w[0]).log2().abs() * 12.0 > 4.0)
            .count();
        let mut sorted: Vec<f32> = f0.iter().copied().filter(|v| *v > 0.0).collect();
        sorted.sort_by(|a, b| a.total_cmp(b));
        println!(
            "  pitch of the output: median {:.0} Hz, {jumps} jumps over 4 semitones in {:.1} s voiced",
            sorted.get(sorted.len() / 2).copied().unwrap_or(0.0),
            voiced as f32 / 100.0
        );
        std::mem::forget(rvc);
    }
    wav::write_mono(&out_path, aligned, SR)?;
    println!("  wrote {}", out_path.display());
    Ok(())
}
