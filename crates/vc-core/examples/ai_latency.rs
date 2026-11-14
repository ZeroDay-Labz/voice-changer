//! Measures the real delay through the AI stage: feeds a voice-like tone in
//! bursts separated by digital silence, at real time, and reports how long
//! after each burst starts the converted voice comes out.
//! `cargo run --release -p vc-core --features ai --example ai_latency -- voice.onnx [auto|cpu] [threads]`
use nice_plug::prelude::Param;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use vc_core::VcParams;
use vc_core::ai::{AiWorker, StreamConfig};

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let mut args = std::env::args().skip(1);
    let voice = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: ai_latency voice.onnx [auto|cpu]"))?;
    let device = args.next().unwrap_or_else(|| "auto".into());

    let params = VcParams::new();
    params.set_ai_voice(&voice);
    params.set_ai_device(&device);
    if let Some(threads) = args.next().and_then(|t| t.parse().ok()) {
        params.set_ai_threads(threads);
    }
    // Switch on via the same internal setter the GUI uses.
    unsafe {
        params
            .ai_enabled
            .as_ptr()
            ._internal_set_normalized_value(1.0);
    }
    let (mut stage, _handle) = AiWorker::spawn(params.clone(), StreamConfig::default());
    let status = stage.status();

    let sr = 48_000usize;
    let quantum = 256usize;
    let period = Duration::from_secs_f64(quantum as f64 / sr as f64);
    let mut buf = vec![0.0f32; quantum];
    let tone = |t: usize| {
        let time = t as f32 / sr as f32;
        let f0 = 130.0 + 20.0 * (2.0 * std::f32::consts::PI * 0.5 * time).sin();
        let mut s = 0.0;
        for h in 1..=12 {
            s += (2.0 * std::f32::consts::PI * f0 * h as f32 * time).sin() / h as f32;
        }
        0.3 * s
    };

    // Phases, in samples of stream time once the stage converts: warm-up
    // tone, then (silence, burst) pairs.
    let warm = sr * 3;
    let gap = sr * 2;
    let burst = sr;
    let rounds = 4;
    let mut t = 0usize; // samples since the stage first converted
    let mut converting = false;
    let mut measured: Vec<f32> = Vec::new();
    let mut waiting_since: Option<usize> = None;
    let mut next = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(120);
    while measured.len() < rounds && Instant::now() < deadline {
        for (i, x) in buf.iter_mut().enumerate() {
            let now = t + i;
            *x = if !converting || now < warm {
                tone(now)
            } else {
                let phase = (now - warm) % (gap + burst);
                if phase == gap {
                    waiting_since = Some(now);
                }
                if phase >= gap { tone(now) } else { 0.0 }
            };
        }
        let processed = stage.process(&mut buf);
        if processed && !converting {
            converting = true;
            t = 0;
        }
        if converting {
            if processed
                && let Some(since) = waiting_since
                && t > since
                && let Some(i) = buf.iter().position(|v| v.abs() > 0.01)
            {
                measured.push((t + i - since) as f32 * 1000.0 / sr as f32);
                waiting_since = None;
            }
            t += quantum;
        }
        next += period;
        if let Some(d) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
    }
    println!(
        "{} | measured delay per burst (ms): {:?} | reported {} ms | infer {:.0} ms/block | dropouts {}",
        status.message(),
        measured
            .iter()
            .map(|m| m.round() as i32)
            .collect::<Vec<_>>(),
        stage.latency_samples() * 1000 / sr,
        status.infer_ms.load(Ordering::Relaxed),
        status.dropouts.load(Ordering::Relaxed),
    );
    Ok(())
}
