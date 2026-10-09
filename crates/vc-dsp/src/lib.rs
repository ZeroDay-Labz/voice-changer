//! Realtime-safe voice processing engine.
//!
//! Everything reachable from [`Engine::process`] must be free of allocation,
//! locking and syscalls. Parameters arrive as a plain [`EngineParams`]
//! snapshot once per block; the engine smooths them internally so hosts
//! don't have to.

#![forbid(unsafe_code)]

pub mod biquad;
pub mod denoise;
pub mod echo;
pub mod engine;
pub mod gate;
pub mod leveler;
pub mod params;
pub mod pitch;
pub mod psola;
pub mod reverb;
pub mod ringmod;
pub mod util;

pub use engine::Engine;
pub use params::{EngineParams, PitchMode};
