//! Minimal WAV reading and writing (mono, PCM16 or float32) for the test
//! bench and the measuring tools.

use anyhow::{Context as _, Result, bail};
use std::path::Path;

/// Read a WAV file, folding all channels to mono. Returns samples and rate.
pub fn read_mono(path: &Path) -> Result<(Vec<f32>, usize)> {
    let b = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let find = |tag: &[u8]| b.windows(4).position(|w| w == tag);
    let fmt = find(b"fmt ").context("wav: no fmt chunk")?;
    if b.len() < fmt + 24 {
        bail!("wav: truncated header");
    }
    let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let format = u16_at(fmt + 8);
    let channels = (u16_at(fmt + 10) as usize).max(1);
    let rate = u32_at(fmt + 12) as usize;
    let bits = u16_at(fmt + 22);
    let data = find(b"data").context("wav: no data chunk")?;
    if b.len() < data + 8 {
        bail!("wav: truncated data chunk");
    }
    let size = u32_at(data + 4) as usize;
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
        _ => bail!("unsupported wav format {format}/{bits} (need 16-bit PCM or 32-bit float)"),
    }
    Ok((mono, rate))
}

/// Write mono 16-bit PCM.
pub fn write_mono(path: &Path, audio: &[f32], rate: usize) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data_len = (audio.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + audio.len() * 2);
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
    std::fs::write(path, b).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = std::env::temp_dir().join(format!("vc-wav-test-{}", std::process::id()));
        let path = dir.join("t.wav");
        let audio: Vec<f32> = (0..480).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        write_mono(&path, &audio, 48_000).unwrap();
        let (back, rate) = read_mono(&path).unwrap();
        assert_eq!(rate, 48_000);
        assert_eq!(back.len(), audio.len());
        for (a, b) in audio.iter().zip(&back) {
            assert!((a - b).abs() < 1e-3);
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
