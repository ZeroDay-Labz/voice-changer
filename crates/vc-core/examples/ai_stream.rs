//! Drives the realtime AI worker the way a host would, at real time, and
//! reports whether it keeps up. `cargo run --release -p vc-core --features ai --example ai_stream -- voice.onnx [seconds]`
use nice_plug::prelude::Param;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use vc_core::VcParams;
use vc_core::ai::{AiWorker, StreamConfig};

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut args = std::env::args().skip(1);
    let voice = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: ai_stream voice.onnx [seconds]"))?;
    let seconds: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(8.0);

    let params = VcParams::new();
    params.set_ai_voice(&voice);
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
    let mut t = 0usize;
    let mut out_nonzero = 0usize;
    let mut out_total = 0usize;
    let start = Instant::now();
    let mut next = start;
    let mut first_audio: Option<Duration> = None;
    while start.elapsed().as_secs_f32() < seconds {
        for x in buf.iter_mut() {
            let time = t as f32 / sr as f32;
            let f0 = 130.0 + 20.0 * (2.0 * std::f32::consts::PI * 0.5 * time).sin();
            let mut s = 0.0;
            for h in 1..=12 {
                s += (2.0 * std::f32::consts::PI * f0 * h as f32 * time).sin() / h as f32;
            }
            *x = 0.3 * s;
            t += 1;
        }
        let processed = stage.process(&mut buf);
        if processed {
            let nz = buf.iter().filter(|v| v.abs() > 1e-4).count();
            out_total += buf.len();
            out_nonzero += nz;
            if nz > 0 && first_audio.is_none() {
                first_audio = Some(start.elapsed());
            }
        }
        next += period;
        if let Some(d) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
    }
    println!(
        "state={:?} msg={:?} infer={:.0} ms/block ({:.0}% load) dropouts={} first_audio_after={:?} nonzero_out={:.1}% latency_report={} ms",
        status.state(),
        status.message(),
        status.infer_ms.load(Ordering::Relaxed),
        status.load_factor() * 100.0,
        status.dropouts.load(Ordering::Relaxed),
        first_audio,
        100.0 * out_nonzero as f32 / out_total.max(1) as f32,
        stage.latency_samples() * 1000 / sr,
    );
    Ok(())
}
