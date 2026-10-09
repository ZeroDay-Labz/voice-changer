# Changelog

All notable changes to Voice Changer are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.2.0] - 2026-10-09

### Added
- NVIDIA GPU support. The same binary now registers ONNX Runtime's CUDA provider when
  an NVIDIA card and Microsoft's CUDA runtime (`scripts/get-onnxruntime.sh --cuda`) are
  present; the Compute picker lists the card as `GPU n · name (CUDA)` and Auto picks it.
  Built and reviewed on an AMD machine; please report how it behaves on yours.

### Changed
- The Cargo feature is now `gpu` (covering ROCm and CUDA); `gpu-rocm` remains as an alias.

## [0.1.0] - 2026-10-08

First public release.

### Added
- Standalone app publishing a PipeWire virtual microphone ("Voice Changer Mic") with
  a hear-myself monitor, tray icon, global shortcut (XDG portal), D-Bus control and CLI.
- **Natural** pitch engine (TD-PSOLA, pitch-synchronous) plus Fast / Balanced / Smooth
  phase-vocoder modes; independent formant shift.
- Effects: drive, robot (ring modulation), echo, reverb, three-band tone, soft limiter,
  each panel with a Reset button.
- Mic cleanup: RNNoise suppression, voice-only gate with adjustable floor, auto level,
  classic noise gate.
- Factory presets (Natural, Man, Woman, Baritone, Bass, Deep, Demon, Monster, Chipmunk,
  Robot, Radio, Cave, Announcer), user presets, favourites and search.
- Local AI voice conversion with RVC v2 models on ONNX Runtime: ContentVec + RMVPE,
  streaming with SOLA joins, automatic speed back-off, breathiness control, "AI only"
  layering switch, FAISS retrieval-index blending (pure Rust reader), shared base-model
  sessions so switching voices takes about two seconds on a GPU.
- Multi-speaker voices: a Speakers setting per voice, a Speaker picker, a pinned
  "Starts as" speaker and speaker names (`voice-changer ai <voice> <speaker>`).
- Compute selection: Auto, CPU or a specific AMD GPU. GPU support is in the default build;
  the app loads the distribution's ROCm ONNX Runtime when present and a bundled CPU runtime
  otherwise, and Settings shows which one is loaded.
- Voice library: import from a Hugging Face page or a direct `.pth` / `.onnx` / `.zip`
  link, automatic `.pth` → ONNX conversion, index pickup, delete.
- "Send the voice to": make Voice Changer Mic the default microphone for every application,
  or tick individual recording applications (Discord, OBS, …). Also `voice-changer apps`,
  `route <app> on|off`, `default on|off`.
- iced user interface shared by the app and the CLAP plugin: Home (fits a 1120×800 window),
  Voices, Effects, Mixer and Settings pages, rotary knobs with drag/wheel/double-click
  reset, live meters with peak readout, hover help on every control, keyboard shortcuts,
  dark and light themes, a colour per page and effect, remembered window geometry.
- The launcher and icons install themselves per user on first start, so the Wayland task
  bar shows the real icon even from `cargo run` or a tarball.
- CLAP plugin (VST3 behind a feature flag), validated with clap-validator.
- Packaging: RPM, .deb, tarball (with the CPU ONNX Runtime), desktop entry, AppStream
  metadata, Flatpak draft, CI and release workflows.
