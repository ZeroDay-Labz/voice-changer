//! Slides the sound-event classifier over WAV files the way the worker
//! does (one look per block) and prints what it hears over time.
//! `cargo run -p vc-core --features ai --example events_probe -- yamnet.onnx block_ms a.wav ...`
use std::path::PathBuf;
use vc_core::ai::events::{Blocker, EventDetector};
use vc_core::ai::resample::Resampler;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let model = PathBuf::from(args.next().expect("model path"));
    let block_ms: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(200.0);
    let mut det = EventDetector::load(&model)?;
    for path in args {
        let (audio, rate) = vc_core::ai::wav::read_mono(std::path::Path::new(&path))?;
        let mut a16 = Vec::new();
        Resampler::new(rate, 16_000).process(&audio, &mut a16);
        let hop = (block_ms * 16.0) as usize;
        let mut blocker = Blocker::default();
        let mut line = String::new();
        let mut muted = 0usize;
        let mut blocks = 0usize;
        let t0 = std::time::Instant::now();
        let mut end = hop;
        while end <= a16.len() {
            let heard = det.listen(&a16[..end])?;
            let mute = blocker.decide(&heard);
            muted += mute as usize;
            blocks += 1;
            line.push_str(&format!(
                "{}{:.0}{}",
                if mute { "[" } else { " " },
                (heard.blocked * 9.0).min(9.0),
                if mute { "]" } else { " " }
            ));
            end += hop;
        }
        let name = std::path::Path::new(&path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        println!(
            "{name:<32} muted {muted:>2}/{blocks:<2} ({:.1} ms/look)  {line}",
            t0.elapsed().as_secs_f32() * 1000.0 / blocks.max(1) as f32
        );
    }
    Ok(())
}
