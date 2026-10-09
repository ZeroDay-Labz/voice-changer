//! Non-realtime glue shared by the plugin and the standalone app.

#[cfg(feature = "ai")]
pub mod ai;
pub mod meters;
pub mod params;
pub mod pipeline;
pub mod presets;

pub use meters::Meters;
pub use params::{AiSpeed, PitchEngine, VcParams};
pub use pipeline::Pipeline;
pub use presets::{Preset, PresetEntry};
pub use vc_dsp;
