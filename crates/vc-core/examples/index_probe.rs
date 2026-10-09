//! Parse an RVC .index file and sanity-check retrieval.
//! `cargo run --release -p vc-core --features ai --example index_probe -- model.index`
use std::time::Instant;
use vc_core::ai::RetrievalIndex;

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: index_probe file.index"))?;
    let t0 = Instant::now();
    let idx = RetrievalIndex::load(std::path::Path::new(&path))?;
    println!(
        "parsed in {:.0} ms: d={} ntotal={} memory={:.0} MB",
        t0.elapsed().as_secs_f32() * 1000.0,
        idx.d,
        idx.ntotal,
        idx.memory_bytes() as f32 / 1e6
    );
    // Self-query: a stored vector must come back unchanged at rate 1.
    let probe = idx.sample_vector(0).expect("vector");
    let mut feats = probe.clone();
    let t1 = Instant::now();
    for _ in 0..100 {
        idx.blend(&mut feats, 1.0, 1, 8);
    }
    let per_frame_us = t1.elapsed().as_secs_f32() * 1e6 / 100.0;
    let err: f32 = feats
        .iter()
        .zip(&probe)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    println!("self-query max error {err:.2e}, {per_frame_us:.0} µs per frame");
    // A random-ish vector gets pulled toward the manifold: distance shrinks.
    let mut noisy: Vec<f32> = probe
        .iter()
        .enumerate()
        .map(|(i, v)| v + 0.3 * ((i % 7) as f32 - 3.0))
        .collect();
    let before: f32 = noisy.iter().zip(&probe).map(|(a, b)| (a - b).powi(2)).sum();
    idx.blend(&mut noisy, 0.75, 1, 8);
    let after: f32 = noisy.iter().zip(&probe).map(|(a, b)| (a - b).powi(2)).sum();
    println!("noisy vector distance to its origin: {before:.2} → {after:.2}");
    Ok(())
}
