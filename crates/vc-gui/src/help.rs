//! Hover explanations for every control, keyed by the parameter's display
//! name (unique in `VcParams`) or a control key.

/// Help for a parameter, by its `Param::name()`.
pub fn param(name: &str) -> &'static str {
    match name {
        "Bypass" => "Off passes your microphone through untouched.",
        "Input Gain" => {
            "Microphone level before any processing. Aim for peaks around −12 dB on the In meter."
        }
        "Output Gain" => "Level sent to the virtual microphone and the monitor.",
        "Limiter" => "Soft limiter on the output. Stops loud peaks from clipping; leave it on.",
        "Noise Suppression" => {
            "RNNoise neural noise suppression. Removes fans, keyboards and hum before anything else."
        }
        "Voice Only" => {
            "Mutes everything the speech detector does not consider voice, so breaths and background sounds never reach the voice engine."
        }
        "Voice Gate Depth" => {
            "How much is let through between words when Voice only is on. −60 dB is silence, −20 dB keeps some room tone."
        }
        "Auto Level" => {
            "Keeps your speech at a steady loudness so quiet and loud passages sound even."
        }
        "Noise Gate" => {
            "Classic gate: closes when the input falls below the threshold. Useful with Voice only off."
        }
        "Gate Threshold" => "Level below which the noise gate closes.",
        "Pitch Engine" => {
            "Natural keeps your voice human for small to medium shifts. Fast, Balanced and Smooth are phase-vocoder modes: lower latency, more robotic on large shifts."
        }
        "Pitch" => "Shift your voice up or down in semitones. ±12 is one octave.",
        "Formant" => {
            "Character of the vocal tract, independent of pitch. Up for a smaller, brighter voice; down for a bigger, darker one."
        }
        "Drive" => "Saturation and grit. Small amounts add presence; large amounts distort.",
        "Ring Mix" => "Amount of ring modulation: the metallic, robotic colour.",
        "Ring Freq" => "Ring modulator frequency. Low values growl, high values ring.",
        "Echo Mix" => "Echo amount.",
        "Echo Time" => "Delay between echoes.",
        "Echo Feedback" => "How many repeats you hear; high values ring on for a long time.",
        "Reverb Mix" => "Reverb amount: the sense of space.",
        "Reverb Size" => "Room size, from a closet to a cathedral.",
        "Reverb Damp" => {
            "High-frequency damping of the reverb tail. More damping sounds warmer and darker."
        }
        "EQ Low" => "Tone: bass below about 200 Hz.",
        "EQ Mid" => "Tone: presence around 1 kHz.",
        "EQ High" => "Tone: air above about 5 kHz.",
        "AI Voice" => "Run the selected AI voice model on your speech.",
        "AI Pitch" => {
            "Pitch offset for the AI voice in semitones. +12 for a typical male→female model, 0 or −12 the other way. Leave the DSP Pitch at 0."
        }
        "AI Speed" => {
            "Block size for the AI worker. Auto starts fast and backs off to longer blocks if the computer cannot keep up."
        }
        "AI Breathiness" => {
            "Scales the model's noise excitation. Lower for a cleaner voice, higher for more texture."
        }
        "AI Only" => {
            "While the AI voice runs, bypass the DSP voice and effects. Turn off to layer Robot, reverb or extra pitch on top."
        }
        "Index Retrieval" => {
            "Blend features toward the voice's retrieval index for a closer timbre match."
        }
        "AI Speaker" => {
            "Which of the model's speakers to use. Some voices were trained with two or more (for example a male and a female take)."
        }
        "Index Strength" => {
            "How strongly to follow the retrieval index. Around 75% keeps the model's timbre; lower if words become unclear."
        }
        _ => "",
    }
}

pub const POWER_ON: &str =
    "Voice changer is on. Click (or press Ctrl+M) to pass the mic through untouched.";
pub const POWER_OFF: &str =
    "Voice changer is off: your mic passes through untouched. Click to turn it on.";
pub const HEAR_MYSELF: &str = "Play the processed voice through your default output so you hear what others hear. Use headphones.";
pub const MIC_PICKER: &str = "Microphone to process. System default follows your desktop's choice.";
pub const PRESET_PILL: &str =
    "Current preset. Pick another on the Home page; Custom means you changed something since.";
pub const SEARCH: &str = "Filter presets by name or description.";
pub const SAVE_PRESET: &str = "Store the current settings as a new preset.";
pub const FAVOURITE: &str = "Favourite: pins this preset to the front.";
pub const DELETE_PRESET: &str = "Delete this user preset.";
pub const VOICE_PICKER: &str = "AI voice model to use. Add more on the Voices page.";
pub const RESCAN: &str = "Rescan the voices folder.";
pub const COMPUTE: &str = "Where the AI models run. Auto picks a GPU when one is available, otherwise the CPU. Models reload when you change it.";
pub const MANAGE_VOICES: &str = "Import, list and delete AI voice models.";
pub const IMPORT_URL: &str = "A Hugging Face model page or a direct .pth / .onnx / .zip link.";
pub const IMPORT_NAME: &str = "Optional display name for the imported voice.";
pub const USE_VOICE: &str = "Select this voice and switch the AI voice on.";
pub const DELETE_VOICE: &str = "Delete this voice (and its index) from disk.";
pub const INDEX_BADGE: &str = "This voice has a retrieval index: closer timbre match.";
pub const DOWNLOAD_BASE: &str =
    "Download ContentVec and RMVPE (about 740 MB, once). AI voices cannot run without them.";
pub const CLOSE_TO_TRAY: &str =
    "When on, the window's X hides Voice Changer in the tray instead of quitting.";
pub const START_MINIMIZED: &str = "Start in the tray without opening the window.";
pub const AUTOSTART: &str = "Start Voice Changer (minimized) when you log in.";
pub const HOTKEY: &str = "Global on/off shortcut through the desktop portal. Works from any app.";
pub const THEME: &str = "Dark or light appearance.";
pub const OPEN_DATA: &str = "Open the folder with models, voices and presets.";
pub const QUIT: &str = "Quit Voice Changer. The virtual microphone disappears.";
pub const DEFAULT_SOURCE: &str = "Take over as the microphone everywhere: every app recording now (even one pinned to a specific device, like Discord) is switched to the voice changer, and apps that open later follow. Each app and your default go back to their own microphone when you turn this off or quit.";
pub const ROUTE_APP: &str = "Send the voice to this application's recording stream. Untick to let the session manager restore its own choice.";
pub const REFRESH_APPS: &str = "Refresh the list of recording applications.";
pub const SPEAKER_COUNT: &str = "How many speakers this model was trained with. RVC files do not record it, so set it yourself: 2 for a voice with a male and a female take. A Speaker picker then appears on the Home page.";
pub const STARTS_AS: &str = "The speaker this voice starts with whenever you select it.";
pub const SPEAKER_NAME: &str =
    "Name this speaker (for example Male or Female). Press Enter to save.";
pub const RESET_PANEL: &str = "Reset every control in this panel to its default.";
pub const STATUS_PILL: &str = "Added delay from mic to virtual mic, dropouts since start, and how hard the AI is working. Click for Settings.";
pub const RUNTIME: &str = "Which ONNX Runtime library the AI uses. The ROCm one enables the GPU.";
