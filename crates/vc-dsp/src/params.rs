/// Which pitch/formant engine configuration is in the signal path.
///
/// Each level is a different Signalsmith Stretch block size, trading
/// latency for low-frequency accuracy. `Off` removes the stretcher from the
/// path entirely (zero added latency).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PitchMode {
    Off,
    /// ~21 ms at 48 kHz. Snappiest; some phasiness on deep voices.
    LowLatency,
    /// ~43 ms at 48 kHz. Natural on speech; good default for chat.
    #[default]
    Balanced,
    /// ~85 ms at 48 kHz. Smoothest; for recording and streaming.
    HighQuality,
    /// Time-domain PSOLA, ~45 ms. Keeps the voice's waveform intact: the
    /// most natural option for speech, a little rough on extreme shifts.
    Natural,
}

impl PitchMode {
    pub const ALL: [PitchMode; 5] = [
        PitchMode::Off,
        PitchMode::LowLatency,
        PitchMode::Balanced,
        PitchMode::HighQuality,
        PitchMode::Natural,
    ];

    /// Stretch (block_length, interval) in samples at 48 kHz, scaled by rate.
    pub fn block_interval(self, sample_rate: f32) -> Option<(usize, usize)> {
        let scale = sample_rate / 48_000.0;
        let pair = match self {
            PitchMode::Off | PitchMode::Natural => return None,
            PitchMode::LowLatency => (1024.0, 256.0),
            PitchMode::Balanced => (2048.0, 512.0),
            PitchMode::HighQuality => (4096.0, 1024.0),
        };
        Some(((pair.0 * scale) as usize, (pair.1 * scale) as usize))
    }

    /// Added latency of this mode in samples (input + output latency of the
    /// stretcher, which together equal one block length).
    pub fn latency_samples(self, sample_rate: f32) -> usize {
        match self {
            PitchMode::Natural => crate::psola::Psola::new(sample_rate).latency_samples(),
            _ => self
                .block_interval(sample_rate)
                .map(|(b, _)| b)
                .unwrap_or(0),
        }
    }
}

/// Block-constant parameter snapshot handed to the engine.
///
/// Gains are linear (not dB) unless the field name says `_db`. The engine
/// applies its own short smoothing so a snapshot can change arbitrarily
/// between blocks without zipper noise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EngineParams {
    /// When true the engine crossfades to the (latency-compensated) dry signal.
    pub bypass: bool,
    pub input_gain: f32,
    pub output_gain: f32,

    pub gate_enabled: bool,
    pub gate_threshold_db: f32,
    /// RNNoise suppression on the input (adds 10 ms).
    pub denoise: bool,
    /// Mute when the noise suppressor hears no speech.
    pub voice_only: bool,
    /// How deep the voice gate attenuates (dB, negative).
    pub gate_floor_db: f32,
    /// Automatic input level riding.
    pub auto_level: bool,

    pub pitch_mode: PitchMode,
    pub pitch_semitones: f32,
    pub formant_semitones: f32,

    /// Saturation amount, 0 = clean.
    pub drive: f32,

    /// 0 = off, 1 = fully ring-modulated.
    pub ring_mix: f32,
    pub ring_freq_hz: f32,

    pub echo_mix: f32,
    pub echo_time_ms: f32,
    pub echo_feedback: f32,

    pub reverb_mix: f32,
    pub reverb_size: f32,
    pub reverb_damp: f32,

    pub eq_low_db: f32,
    pub eq_mid_db: f32,
    pub eq_high_db: f32,

    pub limiter_enabled: bool,
    /// Bypass the pitch engine and effects (used while an AI voice is the
    /// whole point and the DSP chain would just colour it).
    pub skip_voice_fx: bool,
}

impl Default for EngineParams {
    fn default() -> Self {
        Self {
            bypass: false,
            input_gain: 1.0,
            output_gain: 1.0,
            gate_enabled: false,
            gate_threshold_db: -50.0,
            denoise: true,
            voice_only: true,
            gate_floor_db: -40.0,
            auto_level: true,
            pitch_mode: PitchMode::Natural,
            pitch_semitones: 0.0,
            formant_semitones: 0.0,
            drive: 0.0,
            ring_mix: 0.0,
            ring_freq_hz: 80.0,
            echo_mix: 0.0,
            echo_time_ms: 250.0,
            echo_feedback: 0.3,
            reverb_mix: 0.0,
            reverb_size: 0.6,
            reverb_damp: 0.5,
            eq_low_db: 0.0,
            eq_mid_db: 0.0,
            eq_high_db: 0.0,
            limiter_enabled: true,
            skip_voice_fx: false,
        }
    }
}
