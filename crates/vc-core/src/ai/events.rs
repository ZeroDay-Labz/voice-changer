//! Non-speech sounds: coughs, burps, sneezes, throat clearing, hiccups.
//!
//! A voice model turns these into loud, speech-like noise in the target
//! voice. YAMNet (Google's AudioSet classifier, Apache-2.0) recognises them;
//! the worker asks it about the last second of microphone audio before each
//! block and silences the block when one is there.
//!
//! The model is optional: without `yamnet.onnx` in the models folder the
//! blocker simply stays off.

use anyhow::{Context as _, Result, anyhow};
use ort::session::Session;
use ort::value::Tensor;
use std::path::{Path, PathBuf};

/// File name in the models folder.
pub const MODEL_FILE: &str = "yamnet.onnx";
/// Where it comes from: a plain tf2onnx conversion of Google's YAMNet.
pub const MODEL_REPO: &str = "audiomagic/yamnet-onnx";
/// `(file, bytes, sha256)` for the verified download.
pub const MODEL: (&str, u64, &str) = (
    MODEL_FILE,
    16_093_355,
    "d3835ffbbd4a1bb3e777f0ca217b5007907f5171dd5d17c4236b95b2af8f908e",
);

/// One YAMNet patch: 0.975 s at 16 kHz.
pub const WINDOW: usize = 15_600;
const N_CLASSES: usize = 521;
/// The "newest audio on its own" look: 0.6 s.
const RECENT: usize = 9_600;

/// AudioSet class numbers (from `yamnet_class_map.csv`).
const SPEECH: usize = 0;
/// Cough, throat clearing, sneeze, burping, hiccup. Laughter (13) is left
/// to the voice model on purpose.
const BLOCKED: [usize; 5] = [42, 43, 44, 53, 54];

/// What the classifier heard in the last second.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Heard {
    pub speech: f32,
    /// Highest score among the blocked classes, and which class it was.
    pub blocked: f32,
    pub class: usize,
}

impl Heard {
    pub fn class_name(&self) -> &'static str {
        match self.class {
            42 => "cough",
            43 => "throat clearing",
            44 => "sneeze",
            53 => "burp",
            54 => "hiccup",
            _ => "noise",
        }
    }
}

pub fn model_path() -> Option<PathBuf> {
    super::models_dir()
        .map(|d| d.join(MODEL_FILE))
        .filter(|p| p.is_file())
}

pub struct EventDetector {
    /// Never dropped in GPU builds: tearing a session down inside the ROCm
    /// build of ONNX Runtime corrupts its heap (see `stream::release`).
    session: std::mem::ManuallyDrop<Session>,
    window: Vec<f32>,
}

impl EventDetector {
    pub fn load(path: &Path) -> Result<Self> {
        super::rvc::ensure_runtime()?;
        let session = Session::builder()
            .map_err(|e| anyhow!("{e}"))?
            .with_intra_threads(1)
            .map_err(|e| anyhow!("{e}"))?
            .commit_from_file(path)
            .with_context(|| format!("loading {}", path.display()))?;
        Ok(Self {
            session: std::mem::ManuallyDrop::new(session),
            window: vec![0.0; WINDOW],
        })
    }

    /// What is in the newest second of 16 kHz audio. Two looks: the whole
    /// second (a cough stays recognised to its end) and the newest 0.6 s on
    /// its own (a cough that has only just begun is not drowned out by the
    /// speech before it, which makes the start one block earlier).
    pub fn listen(&mut self, audio16k: &[f32]) -> Result<Heard> {
        let full = self.classify(audio16k)?;
        let recent = self.classify(&audio16k[audio16k.len().saturating_sub(RECENT)..])?;
        Ok(if recent.blocked > full.blocked {
            recent
        } else {
            full
        })
    }

    /// Classify the last [`WINDOW`] samples (shorter input is padded with
    /// silence in front).
    fn classify(&mut self, audio16k: &[f32]) -> Result<Heard> {
        let take = audio16k.len().min(WINDOW);
        self.window[..WINDOW - take].fill(0.0);
        self.window[WINDOW - take..].copy_from_slice(&audio16k[audio16k.len() - take..]);
        let input = Tensor::from_array((vec![WINDOW as i64], self.window.clone()))?;
        let outputs = self.session.run(ort::inputs!["waveform" => input])?;
        let (shape, scores) = outputs[0].try_extract_tensor::<f32>()?;
        let classes = *shape.last().unwrap_or(&0) as usize;
        if classes != N_CLASSES || scores.len() < N_CLASSES {
            return Err(anyhow!("unexpected classifier output {shape:?}"));
        }
        // The last row is the patch that ends at the newest sample.
        let row = &scores[scores.len() - N_CLASSES..];
        let (class, blocked) = BLOCKED
            .iter()
            .map(|&c| (c, row[c]))
            .fold((0, 0.0f32), |a, b| if b.1 > a.1 { b } else { a });
        Ok(Heard {
            speech: row[SPEECH],
            blocked,
            class,
        })
    }
}

impl Drop for EventDetector {
    fn drop(&mut self) {
        #[cfg(not(feature = "gpu"))]
        // SAFETY: dropped exactly once, here.
        unsafe {
            std::mem::ManuallyDrop::drop(&mut self.session)
        };
    }
}

/// Turns scores into mute decisions, block by block, with hysteresis so a
/// cough is muted to its end and not just while it is loudest.
#[derive(Debug, Default)]
pub struct Blocker {
    muting: bool,
}

/// Score at which muting starts...
pub const START: f32 = 0.30;
/// ...and below which it ends.
pub const STOP: f32 = 0.12;

impl Blocker {
    /// Whether the block that goes with this observation is muted.
    pub fn decide(&mut self, heard: &Heard) -> bool {
        let threshold = if self.muting { STOP } else { START };
        self.muting = heard.blocked >= threshold;
        self.muting
    }

    pub fn reset(&mut self) {
        self.muting = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heard(blocked: f32) -> Heard {
        Heard {
            speech: 0.5,
            blocked,
            class: 42,
        }
    }

    #[test]
    fn mutes_from_the_start_threshold_until_the_score_has_fallen() {
        let mut b = Blocker::default();
        assert!(!b.decide(&heard(0.2)));
        assert!(b.decide(&heard(0.5)));
        // Still above the lower threshold: the tail of the cough.
        assert!(b.decide(&heard(0.2)));
        assert!(!b.decide(&heard(0.05)));
        // And 0.2 alone does not start it again.
        assert!(!b.decide(&heard(0.2)));
    }
}
