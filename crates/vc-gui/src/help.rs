//! Hover explanations for every control, keyed by the parameter's display
//! name (unique in `VcParams`) or a control key. A test checks that every
//! parameter has one.

/// Help for a parameter, by its `Param::name()`.
pub fn param(name: &str) -> &'static str {
    match name {
        "Bypass" => "Off passes your microphone through untouched.",
        "Mute" => "Silences everything you send, until you unmute.",
        "Input Gain" => {
            "Microphone level before any processing. Aim for peaks around −12 dB on the In meter."
        }
        "Output Gain" => "How loud the changed voice comes out. Keep the Out meter out of the red.",
        "Limiter" => "Soft limiter on the output. Stops loud peaks from clipping; leave it on.",
        "Noise Suppression" => {
            "Removes fans, keyboards and hum before anything else touches your voice."
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
            "How the pitch is changed. Natural keeps your voice human for small to medium shifts. Fast, Balanced and Smooth suit big shifts and sound more synthetic; the number is the delay each one adds. Off adds none and disables Pitch and Character."
        }
        "Pitch" => "Shift your voice up or down in semitones. ±12 is one octave.",
        "Formant" => {
            "The size of the voice, independent of pitch (formant). Up for a smaller, brighter voice; down for a bigger, darker one."
        }
        "Drive" => "Saturation and grit. Small amounts add presence; large amounts distort.",
        "Robot" => "How much of the metallic, robotic colour to mix in (ring modulation).",
        "Robot Freq" => "The robot's tone. Low values growl, high values ring.",
        "Echo" => "Echo amount.",
        "Echo Time" => "Delay between echoes.",
        "Echo Feedback" => "How many repeats you hear; high values ring on for a long time.",
        "Reverb" => "Reverb amount: the sense of space.",
        "Room Size" => "Room size, from a closet to a cathedral.",
        "Damping" => "How quickly the room swallows the highs. More sounds warmer and darker.",
        "Low (200 Hz)" => "Tone: bass and body, around 200 Hz and below.",
        "Mid (1 kHz)" => "Tone: presence, around 1 kHz.",
        "High (4 kHz)" => "Tone: clarity and air, around 4 kHz and above.",
        "AI Voice" => "Run the selected AI voice model on your speech.",
        "AI Pitch" => {
            "Moves the AI voice up or down in semitones. +12 suits a typical male speaker using a female voice, 0 or −12 the other way round. Leave the normal Pitch at 0."
        }
        "AI Speed" => {
            "How much delay to trade for stability. Auto starts with the shortest delay and adds some if the computer cannot keep up."
        }
        "AI Breathiness" => {
            "How airy the AI voice sounds. Lower for a cleaner voice, higher for more texture."
        }
        "AI Only" => {
            "While the AI voice runs, skip the normal voice changes and effects. Turn off to add Robot, reverb or extra pitch on top of the AI voice."
        }
        "Use Voice Index" => {
            "This voice came with an index file: using it makes the result sound closer to the original voice."
        }
        "AI Speaker" => {
            "Which of the model's speakers to use. Some voices were trained with two or more (for example a male and a female take)."
        }
        "Protect Consonants" => {
            "Keeps breaths and consonants (s, t, sh) closer to what you actually said. Higher is cleaner; 0% is off."
        }
        "Follow My Loudness" => {
            "Makes the converted voice as loud, moment by moment, as what you put in. Quiet breaths and mouth noises stay quiet instead of coming out at full speech level."
        }
        "Block Coughs and Burps" => {
            "Silences coughs, burps, sneezes, throat clearing and hiccups instead of converting them. Laughter is left to the voice. Needs a small extra model (15 MB)."
        }
        "Index Strength" => {
            "How strongly to follow the voice's index. Around 75% keeps the voice's character; lower it if words become unclear."
        }
        _ => "",
    }
}

pub const POWER_ON: &str = "The voice changer is on. Click (or press Ctrl+M) to switch it off: your own voice then goes through unchanged. To send nothing at all, use Mute.";
pub const POWER_OFF: &str = "The voice changer is off: your own voice goes through unchanged. Click (or press Ctrl+M) to turn it on.";
pub const MUTE: &str =
    "Silence everything you send: a cough button. Click again (or press Ctrl+U) to unmute.";
pub const MUTED: &str = "You are muted: nothing is sent. Click (or press Ctrl+U) to unmute.";
pub const HEAR_MYSELF: &str = "Play the changed voice in your headphones so you hear what others hear (Ctrl+H). Use headphones, not speakers.";
pub const METER_IN: &str = "Your microphone's level. Speak normally: the bar should reach the marked zone and never stay red.";
pub const METER_OUT: &str = "The level of the changed voice going out. Red means it is too loud and distorts: turn Output down on the Mixer page.";
pub const NO_PRESETS_FOUND: &str = "No preset matches. Clear the search to see them all.";
pub const UPDATE_PRESET: &str =
    "Store the current settings in this preset, replacing what it held.";
pub const RENAME_PRESET: &str = "Give this preset another name.";
pub const IMPORT_PRESET: &str = "Add a preset file (.ron) someone shared with you.";
pub const EXPORT_PRESET: &str = "Save this preset as a file you can share or back up.";
pub const IMPORT_FILE: &str = "Add a voice from a file on this computer (.onnx, .pth or .zip).";
pub const SHOW_WELCOME: &str = "Show the first steps card on the Home page again.";
pub const UI_SCALE: &str = "Make everything in the window larger or smaller.";
pub const BUFFER: &str = "How much audio is handled at a time. Larger survives a busy computer without crackle and adds a little delay. Takes effect the next time Voice Changer starts.";
pub const MIC_PICKER: &str = "Microphone to process. System default follows your desktop's choice.";
pub const OUTPUT_PICKER: &str = "Where your changed voice plays: headphones, speakers, a mixer or an audio interface. System default follows your computer's choice.";
pub const OUTPUT_PICKER_MONITOR: &str = "Where Hear myself plays. System default follows your desktop's choice. Applications get the voice through Voice Changer Mic, whatever you pick here.";
pub const PRESET_PILL: &str =
    "Current preset. Pick another on the Home page; Custom means you changed something since.";
pub const SEARCH: &str = "Filter presets by name or description.";
pub const SAVE_PRESET: &str = "Store the current settings as a preset of your own (Ctrl+S). Using the name of one of your presets replaces it.";
pub const FAVOURITE: &str = "Favourite: pins this preset to the front.";
pub const DELETE_PRESET: &str = "Delete this preset.";
pub const EDIT_PRESETS: &str = "Rename, share or delete your own presets, or add one from a file.";
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
pub const HOTKEY: &str = "A shortcut that switches the voice changer on and off from any program.";
pub const THEME: &str = "Dark or light appearance, or whatever the desktop uses.";
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
pub const CHECK_VOICE: &str = "Convert a short test phrase with this voice and report clipping, silence, poor pitch tracking or a missing index. Takes a few seconds.";
pub const VOICE_SETTINGS: &str = "Pitch, index, breathiness, protection and loudness were set for this voice and come back when you select it.";
pub const STATUS_PILL: &str = "Added delay from mic to virtual mic, dropouts since start, and how hard the AI is working. Click for Settings.";
pub const RUNTIME: &str =
    "The AI engine library in use. The ROCm or CUDA one lets the AI run on your graphics card.";
pub const AI_THREADS: &str = "How many processor threads the AI may use. Auto keeps one core free for audio and the window and never exceeds what your machine has. Lower it if other programs stutter while the AI runs; raise it if the AI cannot keep up.";
pub const RETRY_AUDIO: &str = "Try to connect to the audio system again.";
pub const WELCOME_DONE: &str =
    "Hide this card. Everything in it stays available on the Settings and Mixer pages.";
pub const LISTENERS: &str = "Applications currently receiving the voice from Voice Changer Mic.";

#[cfg(test)]
mod tests {
    use nice_plug::prelude::*;

    /// Every control must explain itself: a parameter renamed without its
    /// help text being renamed with it would silently lose its tooltip.
    #[test]
    fn every_parameter_has_help() {
        let params = vc_core::VcParams::default();
        let missing: Vec<String> = params
            .param_map()
            .into_iter()
            .map(|(_, ptr, _)| unsafe { ptr.name() }.to_string())
            .filter(|name| super::param(name).is_empty())
            .collect();
        assert!(missing.is_empty(), "no help text for: {missing:?}");
    }
}
