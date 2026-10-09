//! Developer convenience: run the plugin as a JACK/ALSA standalone through
//! nice-plug's own wrapper (`--backend jack`). The real desktop app lives in
//! `vc-app`.
fn main() {
    nice_plug::nice_export_standalone::<vc_plugin::VoiceChangerPlugin>();
}
