//! Non-realtime glue shared by the plugin and the standalone app.

#[cfg(feature = "ai")]
pub mod ai;
pub mod meters;
pub mod params;
pub mod pipeline;
pub mod presets;
pub mod wav;

pub use meters::Meters;
pub use params::{AiSpeed, PitchEngine, VcParams};
pub use pipeline::Pipeline;
pub use presets::{Preset, PresetEntry};
pub use vc_dsp;

/// A command for a helper program (`nvidia-smi`, Python…). On Windows it is
/// started without a console window, which would otherwise flash on the
/// screen every time, since Voice Changer itself has none.
pub fn quiet_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut command = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}
