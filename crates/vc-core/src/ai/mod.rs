//! Local AI voice conversion (RVC-style) on ONNX Runtime.
//!
//! Pipeline per block: 48 kHz mic → 16 kHz → ContentVec features + RMVPE
//! pitch → per-voice synthesizer (`net_g`) → model rate → 48 kHz. The
//! realtime side only touches lock-free ring buffers; all inference runs on
//! a worker thread.

pub mod compute;
pub mod f0;
pub mod index;
pub mod library;
pub mod mel;
pub mod onnx_meta;
pub mod resample;
pub mod rvc;
pub mod stream;

use std::path::{Path, PathBuf};

pub use compute::{Backend, GpuInfo};
pub use index::RetrievalIndex;
pub use library::{ImportStatus, Stage, VoiceInfo};
pub use rvc::{ModelPaths, Rvc};
pub use stream::{AiHandle, AiStage, AiState, AiStatus, AiWorker, StreamConfig};

/// `~/.local/share/voice-changer/models`
pub fn models_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "echo", "voice-changer").map(|d| d.data_dir().join("models"))
}

pub fn voices_dir() -> Option<PathBuf> {
    models_dir().map(|d| d.join("voices"))
}

/// Base models that have to exist for AI mode to work at all. fp32 first:
/// on CPU the int8 exports run 3-4x *slower* with ONNX Runtime 1.28.
pub const CONTENTVEC_FILES: &[&str] = &[
    "contentvec_768l12.onnx",
    "vec-768-layer-12.onnx",
    "contentvec_768l12_q8.onnx",
];
pub const RMVPE_FILES: &[&str] = &["rmvpe.onnx", "rmvpe_q8.onnx"];

fn first_existing(dir: &Path, names: &[&str]) -> Option<PathBuf> {
    names.iter().map(|n| dir.join(n)).find(|p| p.is_file())
}

/// Locate the base models in `dir`, preferring the fp32 variants.
pub fn find_base_models(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    Some((
        first_existing(dir, CONTENTVEC_FILES)?,
        first_existing(dir, RMVPE_FILES)?,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceEntry {
    pub name: String,
    pub path: PathBuf,
}

/// Voice models (`*.onnx`) in the voices folder, sorted by name.
pub fn list_voices() -> Vec<VoiceEntry> {
    let Some(dir) = voices_dir() else {
        return Vec::new();
    };
    let Ok(read) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut v: Vec<VoiceEntry> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "onnx"))
        .map(|path| VoiceEntry {
            name: path
                .file_stem()
                .map(|s| s.to_string_lossy().replace(['_', '-'], " "))
                .unwrap_or_default(),
            path,
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}
