use nice_plug::prelude::*;
use std::sync::Arc;
use vc_dsp::util::db_to_gain;
use vc_dsp::{EngineParams, PitchMode};

/// Whether the AI runs on a GPU build (affects how much context is affordable).
fn vc_dsp_gpu_hint() -> bool {
    cfg!(feature = "gpu-rocm")
}

/// Host-visible mirror of [`PitchMode`] (nice-plug needs its own `Enum` derive).
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PitchEngine {
    #[name = "Off (no latency)"]
    Off,
    #[name = "Fast (~21 ms)"]
    LowLatency,
    #[name = "Balanced (~43 ms)"]
    Balanced,
    #[name = "Smooth (~85 ms)"]
    HighQuality,
    #[name = "Natural (~45 ms)"]
    Natural,
}

/// Block size trade-off for the AI worker: shorter blocks = less delay but
/// the models must finish faster.
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiSpeed {
    #[name = "Auto (backs off if needed)"]
    Auto,
    #[name = "Fast (~0.55 s delay)"]
    Fast,
    #[name = "Normal (~0.7 s delay)"]
    Normal,
    #[name = "Safe (~1 s delay)"]
    Safe,
}

impl AiSpeed {
    /// (block_ms, context_ms). `Auto` starts like `Fast`. With a GPU the
    /// content encoder gets far more context (that is what keeps the timbre
    /// steady from block to block) because it is nearly free there.
    pub fn block_context_ms(self) -> (f32, f32) {
        let gpu = cfg!(feature = "ai") && vc_dsp_gpu_hint();
        match (self, gpu) {
            (AiSpeed::Auto | AiSpeed::Fast, false) => (300.0, 200.0),
            (AiSpeed::Normal, false) => (400.0, 250.0),
            (AiSpeed::Safe, false) => (550.0, 350.0),
            (AiSpeed::Auto | AiSpeed::Fast, true) => (300.0, 1000.0),
            (AiSpeed::Normal, true) => (400.0, 1000.0),
            (AiSpeed::Safe, true) => (550.0, 1000.0),
        }
    }

    /// Next slower setting, if any.
    pub fn slower(self) -> Option<AiSpeed> {
        match self {
            AiSpeed::Auto | AiSpeed::Fast => Some(AiSpeed::Normal),
            AiSpeed::Normal => Some(AiSpeed::Safe),
            AiSpeed::Safe => None,
        }
    }
}

impl From<PitchEngine> for PitchMode {
    fn from(v: PitchEngine) -> Self {
        match v {
            PitchEngine::Off => PitchMode::Off,
            PitchEngine::LowLatency => PitchMode::LowLatency,
            PitchEngine::Balanced => PitchMode::Balanced,
            PitchEngine::HighQuality => PitchMode::HighQuality,
            PitchEngine::Natural => PitchMode::Natural,
        }
    }
}

/// Every user-facing parameter. One instance is shared (via `Arc`) between
/// the audio thread, the UI and the host.
#[derive(Params)]
pub struct VcParams {
    #[id = "bypass"]
    pub bypass: BoolParam,
    #[id = "in_gain"]
    pub input_gain: FloatParam,
    #[id = "out_gain"]
    pub output_gain: FloatParam,
    #[id = "limiter"]
    pub limiter: BoolParam,

    #[id = "denoise"]
    pub denoise: BoolParam,
    #[id = "voice_only"]
    pub voice_only: BoolParam,
    #[id = "gate_floor"]
    pub gate_floor: FloatParam,
    #[id = "auto_level"]
    pub auto_level: BoolParam,
    #[id = "gate_on"]
    pub gate_enabled: BoolParam,
    #[id = "gate_thresh"]
    pub gate_threshold: FloatParam,

    #[id = "pitch_engine"]
    pub pitch_engine: EnumParam<PitchEngine>,
    #[id = "pitch"]
    pub pitch: FloatParam,
    #[id = "formant"]
    pub formant: FloatParam,

    #[id = "drive"]
    pub drive: FloatParam,

    #[id = "ring_mix"]
    pub ring_mix: FloatParam,
    #[id = "ring_freq"]
    pub ring_freq: FloatParam,

    #[id = "echo_mix"]
    pub echo_mix: FloatParam,
    #[id = "echo_time"]
    pub echo_time: FloatParam,
    #[id = "echo_fb"]
    pub echo_feedback: FloatParam,

    #[id = "rev_mix"]
    pub reverb_mix: FloatParam,
    #[id = "rev_size"]
    pub reverb_size: FloatParam,
    #[id = "rev_damp"]
    pub reverb_damp: FloatParam,

    #[id = "eq_low"]
    pub eq_low: FloatParam,
    #[id = "eq_mid"]
    pub eq_mid: FloatParam,
    #[id = "eq_high"]
    pub eq_high: FloatParam,

    /// Run the selected AI voice model ahead of the DSP chain.
    #[id = "ai_on"]
    pub ai_enabled: BoolParam,
    /// Pitch offset applied to the detected f0 before synthesis.
    #[id = "ai_pitch"]
    pub ai_pitch: FloatParam,
    #[id = "ai_speed"]
    pub ai_speed: EnumParam<AiSpeed>,
    /// Scale of the synthesizer's random excitation: lower = cleaner and
    /// less breathy, higher = more natural texture (RVC default ≈ 100%).
    #[id = "ai_breath"]
    pub ai_breath: FloatParam,
    /// While the AI voice is on, bypass the DSP voice and effects.
    #[id = "ai_solo"]
    pub ai_solo: BoolParam,
    /// Use the voice's retrieval index (.index) when present.
    #[id = "ai_index_on"]
    pub ai_index_enabled: BoolParam,
    /// How strongly features are pulled toward the voice's training set.
    #[id = "ai_index_rate"]
    pub ai_index_rate: FloatParam,
    /// Speaker id (`ds`) for multi-speaker voice models.
    #[id = "ai_speaker"]
    pub ai_speaker: IntParam,
    /// Path of the voice model (`.onnx`); empty = none. Not a host
    /// parameter, but saved with the plugin state.
    #[persist = "ai_voice"]
    pub ai_voice: std::sync::RwLock<String>,
    /// Where the models run: "auto", "cpu" or "rocm:N".
    #[persist = "ai_device"]
    pub ai_device: std::sync::RwLock<String>,
}

impl Default for VcParams {
    fn default() -> Self {
        Self {
            bypass: BoolParam::new("Bypass", false).make_bypass(),
            input_gain: db_param("Input Gain", 0.0, -24.0, 24.0),
            output_gain: db_param("Output Gain", 0.0, -24.0, 24.0),
            limiter: BoolParam::new("Limiter", true),

            denoise: BoolParam::new("Noise Suppression", true),
            voice_only: BoolParam::new("Voice Only", true),
            gate_floor: db_param("Voice Gate Depth", -40.0, -80.0, -10.0),
            auto_level: BoolParam::new("Auto Level", true),
            gate_enabled: BoolParam::new("Noise Gate", false),
            gate_threshold: FloatParam::new(
                "Gate Threshold",
                -50.0,
                FloatRange::Linear {
                    min: -80.0,
                    max: 0.0,
                },
            )
            .with_unit(" dB")
            .with_step_size(0.5)
            .with_value_to_string(formatters::v2s_f32_rounded(1)),

            pitch_engine: EnumParam::new("Pitch Engine", PitchEngine::Natural),
            pitch: semitone_param("Pitch", 24.0),
            formant: semitone_param("Formant", 12.0),

            drive: percent_param("Drive", 0.0),
            ring_mix: percent_param("Robot", 0.0),
            ring_freq: FloatParam::new(
                "Robot Freq",
                80.0,
                FloatRange::Skewed {
                    min: 10.0,
                    max: 2000.0,
                    factor: FloatRange::skew_factor(-1.5),
                },
            )
            .with_unit(" Hz")
            .with_step_size(1.0)
            .with_value_to_string(formatters::v2s_f32_rounded(0)),

            echo_mix: percent_param("Echo", 0.0),
            echo_time: FloatParam::new(
                "Echo Time",
                250.0,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 1000.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_unit(" ms")
            .with_step_size(1.0)
            .with_value_to_string(formatters::v2s_f32_rounded(0)),
            echo_feedback: percent_param("Echo Feedback", 0.3),

            reverb_mix: percent_param("Reverb", 0.0),
            reverb_size: percent_param("Room Size", 0.6),
            reverb_damp: percent_param("Damping", 0.5),

            eq_low: db_param("Low (200 Hz)", 0.0, -12.0, 12.0),
            eq_mid: db_param("Mid (1 kHz)", 0.0, -12.0, 12.0),
            eq_high: db_param("High (4 kHz)", 0.0, -12.0, 12.0),

            ai_enabled: BoolParam::new("AI Voice", false),
            ai_pitch: semitone_param("AI Pitch", 24.0),
            ai_speed: EnumParam::new("AI Speed", AiSpeed::Auto),
            ai_breath: percent_param("AI Breathiness", 0.5),
            ai_solo: BoolParam::new("AI Only", true),
            ai_index_enabled: BoolParam::new("Use Voice Index", true),
            ai_index_rate: percent_param("Index Strength", 0.75),
            ai_speaker: IntParam::new("AI Speaker", 0, IntRange::Linear { min: 0, max: 15 }),
            ai_voice: std::sync::RwLock::new(String::new()),
            ai_device: std::sync::RwLock::new("auto".into()),
        }
    }
}

fn db_param(name: &str, default: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min, max })
        .with_unit(" dB")
        .with_step_size(0.1)
        .with_value_to_string(formatters::v2s_f32_rounded(1))
}

fn semitone_param(name: &str, range: f32) -> FloatParam {
    FloatParam::new(
        name,
        0.0,
        FloatRange::Linear {
            min: -range,
            max: range,
        },
    )
    .with_unit(" st")
    .with_step_size(0.1)
    .with_value_to_string(formatters::v2s_f32_rounded(1))
}

fn percent_param(name: &str, default: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: 0.0, max: 1.0 })
        .with_unit("%")
        .with_step_size(0.01)
        .with_value_to_string(formatters::v2s_f32_percentage(0))
        .with_string_to_value(formatters::s2v_f32_percentage())
}

impl VcParams {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn ai_voice(&self) -> String {
        self.ai_voice.read().map(|v| v.clone()).unwrap_or_default()
    }

    pub fn ai_device(&self) -> String {
        self.ai_device
            .read()
            .map(|v| v.clone())
            .unwrap_or_else(|_| "auto".into())
    }

    pub fn set_ai_device(&self, setting: &str) {
        if let Ok(mut v) = self.ai_device.write() {
            *v = setting.to_string();
        }
    }

    pub fn set_ai_voice(&self, path: &str) {
        if let Ok(mut v) = self.ai_voice.write() {
            *v = path.to_string();
        }
    }

    /// Build the block-constant snapshot the engine consumes. Realtime-safe
    /// (atomic loads only).
    pub fn snapshot(&self) -> EngineParams {
        EngineParams {
            bypass: self.bypass.value(),
            input_gain: db_to_gain(self.input_gain.value()),
            output_gain: db_to_gain(self.output_gain.value()),
            gate_enabled: self.gate_enabled.value(),
            gate_threshold_db: self.gate_threshold.value(),
            denoise: self.denoise.value(),
            voice_only: self.voice_only.value(),
            gate_floor_db: self.gate_floor.value(),
            auto_level: self.auto_level.value(),
            pitch_mode: self.pitch_engine.value().into(),
            pitch_semitones: self.pitch.value(),
            formant_semitones: self.formant.value(),
            drive: self.drive.value(),
            ring_mix: self.ring_mix.value(),
            ring_freq_hz: self.ring_freq.value(),
            echo_mix: self.echo_mix.value(),
            echo_time_ms: self.echo_time.value(),
            echo_feedback: self.echo_feedback.value(),
            reverb_mix: self.reverb_mix.value(),
            reverb_size: self.reverb_size.value(),
            reverb_damp: self.reverb_damp.value(),
            eq_low_db: self.eq_low.value(),
            eq_mid_db: self.eq_mid.value(),
            eq_high_db: self.eq_high.value(),
            limiter_enabled: self.limiter.value(),
            skip_voice_fx: false,
        }
    }
}
