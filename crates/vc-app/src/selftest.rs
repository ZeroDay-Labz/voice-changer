//! `voice-changer --self-test`: run the engine on synthetic speech through
//! every preset and fail on NaN, runaway level or silence. Fast (a few
//! seconds), needs no PipeWire, no microphone and no models, so CI and
//! packagers can prove the build works on the machine it runs on.

use anyhow::{Result, bail};
use std::sync::Arc;
use vc_core::{Pipeline, VcParams};

/// A vowel-like signal: a 120 Hz harmonic series shaped by two formants,
/// with slow amplitude movement and a touch of noise, so the voice-only
/// gate and the pitch tracker see something that behaves like speech.
pub fn synth_voice(rate: usize, seconds: f32) -> Vec<f32> {
    let n = (rate as f32 * seconds) as usize;
    let mut out = Vec::with_capacity(n);
    let f0 = 120.0f32;
    let formants = [(500.0f32, 1.0f32), (1500.0, 0.5), (2500.0, 0.25)];
    let mut noise = 0x9E37_79B9_7F4A_7C15u64;
    for i in 0..n {
        let t = i as f32 / rate as f32;
        let mut s = 0.0f32;
        for h in 1..=24 {
            let f = f0 * h as f32 * (1.0 + 0.01 * (t * 3.0).sin());
            let mut amp = 1.0 / h as f32;
            for (fc, g) in formants {
                let d = (f - fc) / 180.0;
                amp += g * (-d * d).exp();
            }
            s += amp * (2.0 * std::f32::consts::PI * f * t).sin();
        }
        noise ^= noise << 13;
        noise ^= noise >> 7;
        noise ^= noise << 17;
        let n = ((noise >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.02;
        let env = 0.6 + 0.4 * (t * 1.3).sin();
        out.push((s * 0.06 + n) * env);
    }
    out
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// Returns Ok when every preset produced finite, non-silent, bounded audio.
pub fn run(rate: u32, quantum: u32) -> Result<()> {
    let rate_us = rate as usize;
    let input = synth_voice(rate_us, 2.0);
    let params: Arc<VcParams> = VcParams::new();
    let mut pipeline = Pipeline::new(rate as f32, 8192, params.clone());
    let presets = vc_core::presets::list_presets();
    if presets.is_empty() {
        bail!("no presets found");
    }
    let mut failures = Vec::new();
    let started = std::time::Instant::now();
    for entry in &presets {
        entry.preset.apply(&params, |ptr, n| unsafe {
            ptr._internal_set_normalized_value(n);
            ptr._internal_update_smoother(rate as f32, true);
        });
        // The synthetic voice is not real speech; judge the engine, not the
        // speech detector.
        set(&params, &params.voice_only, false);
        set(&params, &params.gate_enabled, false);
        set(&params, &params.ai_enabled, false);
        set(&params, &params.bypass, false);
        pipeline.reset();
        let mut out = Vec::with_capacity(input.len());
        let block = (quantum as usize).clamp(32, 8192);
        for chunk in input.chunks(block) {
            let mut buf = chunk.to_vec();
            pipeline.process(&mut buf);
            out.extend_from_slice(&buf);
        }
        // Skip the first half second (gates, lookahead, pitch engine latency).
        let tail = &out[(rate_us / 2).min(out.len())..];
        let level = rms(tail);
        let peak = tail.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let verdict = if tail.iter().any(|v| !v.is_finite()) {
            Some("produced NaN or infinity")
        } else if peak > 4.0 {
            Some("output level exploded")
        } else if level < 1e-4 {
            Some("output is silent")
        } else {
            None
        };
        match verdict {
            None => println!(
                "ok    {:<12} rms {level:.3} peak {peak:.3}",
                entry.preset.name
            ),
            Some(why) => {
                println!(
                    "FAIL  {:<12} rms {level:.3} peak {peak:.3}: {why}",
                    entry.preset.name
                );
                failures.push(format!("{}: {why}", entry.preset.name));
            }
        }
    }
    println!(
        "{} presets in {:.1} s on {}",
        presets.len(),
        started.elapsed().as_secs_f32(),
        std::env::consts::ARCH
    );
    if failures.is_empty() {
        Ok(())
    } else {
        bail!("self-test failed: {}", failures.join("; "))
    }
}

fn set(params: &VcParams, p: &nice_plug::prelude::BoolParam, value: bool) {
    use nice_plug::prelude::*;
    let ptr = p.as_ptr();
    let n = p.preview_normalized(value);
    unsafe {
        ptr._internal_set_normalized_value(n);
        ptr._internal_update_smoother(params.pitch.value().max(1.0), true);
    }
}
