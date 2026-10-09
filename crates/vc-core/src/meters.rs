//! Level meters shared between the audio thread (writer) and the UI (reader).

use nice_plug::prelude::AtomicF32;
use std::sync::atomic::Ordering;

#[derive(Default)]
pub struct Meters {
    /// Peak of the most recent input block, linear 0..1+.
    pub input: AtomicF32,
    /// Peak of the most recent output block.
    pub output: AtomicF32,
    /// Whether the noise gate is currently letting signal through.
    pub gate_open: std::sync::atomic::AtomicBool,
    /// Current auto-level gain in dB.
    pub auto_gain_db: AtomicF32,
}

impl Meters {
    /// Called from the audio thread after each block.
    #[inline]
    pub fn publish(&self, input_peak: f32, output_peak: f32, gate_open: bool) {
        self.input.store(input_peak, Ordering::Relaxed);
        self.output.store(output_peak, Ordering::Relaxed);
        self.gate_open.store(gate_open, Ordering::Relaxed);
    }

    pub fn publish_auto_gain(&self, db: f32) {
        self.auto_gain_db.store(db, Ordering::Relaxed);
    }

    pub fn read(&self) -> (f32, f32, bool) {
        (
            self.input.load(Ordering::Relaxed),
            self.output.load(Ordering::Relaxed),
            self.gate_open.load(Ordering::Relaxed),
        )
    }
}
