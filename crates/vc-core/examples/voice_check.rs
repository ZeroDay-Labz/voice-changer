//! Runs the import quality check on voices and prints the reports.
//! `cargo run -p vc-core --features ai --example voice_check -- [--phrase clip.wav] voice.onnx ...`
use std::path::PathBuf;
use vc_core::ai::check;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut phrase = check::test_phrase();
    let mut label = "synthetic phrase".to_string();
    if args.first().is_some_and(|a| a == "--phrase") {
        let path = PathBuf::from(args.remove(1));
        args.remove(0);
        let (audio, rate) = vc_core::ai::wav::read_mono(&path)?;
        phrase.clear();
        vc_core::ai::resample::Resampler::new(rate, 16_000).process(&audio, &mut phrase);
        phrase.truncate(16_000 * 6);
        label = path.display().to_string();
    }
    println!("phrase: {label}");
    for voice in args {
        let t0 = std::time::Instant::now();
        let r = check::check_voice(std::path::Path::new(&voice), &phrase, 8)?;
        println!(
            "{:<24} {:<5} ({:.1} s)",
            std::path::Path::new(&voice)
                .file_stem()
                .unwrap()
                .to_string_lossy(),
            r.grade,
            t0.elapsed().as_secs_f32()
        );
        for n in r.notes {
            println!("    - {n}");
        }
    }
    Ok(())
}
