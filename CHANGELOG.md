# Changelog

All notable changes to Voice Changer are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.3.1] - 2026-10-11

### Fixed
- **Crash at start with `onnxruntime-rocm` installed but no AMD GPU.** The ROCm build of
  ONNX Runtime aborts (a C++ assertion on an empty device list) as soon as a model is loaded
  on a machine where `rocminfo` finds no GPU. The app now only loads the ROCm runtime when
  a ROCm GPU is present and uses the bundled CPU runtime otherwise; Settings and
  `voice-changer doctor` say so. `ORT_DYLIB_PATH` still forces a specific library.

## [0.3.0] - 2026-10-11

The "runs everywhere, runs smoothly" release.

### Added
- **Portable builds.** The x86_64 release binary is linked against glibc 2.35, so it
  starts on Ubuntu 22.04, Debian 12, Linux Mint 21, Zorin 17 and everything newer
  (previous releases needed glibc 2.39). A native **aarch64** build (Raspberry Pi 5,
  Asahi, ARM laptops) ships alongside, each as tarball, `.rpm` and `.deb`, plus a
  **Flatpak** bundle and an **AUR** recipe (`packaging/aur`). Every release carries a
  `SHA256SUMS` file. The release workflow refuses a binary that needs a newer glibc or
  libstdc++ than that (`scripts/check-portable.sh`).
- **`voice-changer doctor`** checks the machine the way a support person would: is
  PipeWire the sound server (or only PulseAudio?), is WirePlumber running, can virtual
  devices be created, which microphones, GPU and AI runtime are present, are the base
  models and voices installed, is there a tray and a shortcut portal. Each problem comes
  with the command that fixes it.
- **`voice-changer --self-test`** runs the engine through every preset on synthetic
  speech and fails on silence, NaN or runaway levels. CI and packagers use it; the CI
  also starts a private PipeWire session, records the virtual microphone through the
  PulseAudio API the way Discord does, and checks that the voice came out changed.
- **AI threads follow your hardware.** The number of inference threads is derived from
  the processor (physical cores, hybrid P/E cores, SMT) and whether a GPU is used,
  keeping one core free for audio and the window; it used to be a fixed 8, too many for
  a laptop and too few for a workstation. Settings → AI shows what the machine has and
  lets you pick a number, bounded by the threads actually present; `--ai-threads N|auto`
  does the same from the terminal.
- **Welcome card** on the first start: microphone, hear yourself, send it to your apps,
  each with its own live checkmark.
- The sidebar and the Mixer page show **how many apps are hearing the voice**, and say
  what to do when none is.
- An unplugged microphone now **falls back to the system default and comes back** when
  it is plugged in again, with a line in Settings while it is away.

### Fixed
- **Starting without a working audio system no longer exits silently.** When PipeWire is
  missing, refused, or PulseAudio still owns the sound, the window opens with a plain
  explanation and a **Try again** button; headless runs print the same text. Every
  set-up failure inside the audio thread is now reported, not only the first connect.
- **A PipeWire or WirePlumber restart no longer leaves a dead app.** The audio side
  reconnects by itself with a short back-off, re-creates the virtual microphone, and
  re-applies the chosen microphone and "Replace my microphone everywhere". The AI
  models stay loaded across the gap.
- **"Replace my microphone everywhere" saved from a previous run now actually applies**
  at start. It used to race the session manager and silently do nothing; it now waits
  for the session manager and a real microphone, then takes over (and never pins the
  capture to itself, so no feedback loop).
- **A missing or broken AI voice no longer makes the worker retry twenty times a
  second** (and, on GPU builds, leak a model session each time). A failed voice is left
  alone for thirty seconds or until you change something; a voice that keeps failing
  during inference is parked with a clear message.
- Base-model downloads are checked against their size and SHA-256; a cut-off or corrupt
  download is deleted and reported instead of being installed as a model that cannot load.
- ONNX Runtime libraries are compared by version number (an old `1.9` library sorted
  after `1.22` by string order), and libraries older than the API this build needs are
  skipped. GPU probing (`rocminfo`, `nvidia-smi`) runs in the background, so the window
  never waits for it.
- The window no longer freezes on the Settings page when a very long log record arrives
  (iced logs its whole window configuration, icon pixels included; the in-app log now
  keeps one bounded line per record).
- A tray that cannot be created no longer cancels the global shortcut, and
  "Start minimized" without a tray opens the window instead of leaving no way in.
- A crash restores every application's own microphone and the default source before
  the process exits. A capture buffer with a bad offset can no longer panic the
  realtime thread, and underruns are counted rather than logged from it.
- The AI resampler follows the engine sample rate instead of assuming 48 kHz; the
  misleading "need contentvec_768l12_q8.onnx" message names the files actually used.
- With the window closed to the tray the app ticks at 4 Hz instead of 60 Hz.

### Changed
- A second launch brings the running window to the front instead of starting a second
  instance.
- The monitor ("Hear myself") stream is tagged as music rather than communication so a
  Bluetooth headset is not switched to its low-quality profile by a silent stream.
- Technical wording in the window was replaced with plain language; the AI runtime line
  is now "AI engine".

## [0.2.5] - 2026-10-10

### Fixed
- **The AI voice could mute your microphone completely** (it "killed Discord"). While the
  model was warming up -- which happens on every start, voice switch, compute switch and
  error retry -- and on every dropout, the AI stage filled the output with digital silence;
  with "AI only" on, the normal voice effects were switched off at the same time, so nothing
  came out at all. Measured on the virtual microphone, 5 of 20 seconds after start were
  completely silent. Now the live voice keeps flowing whenever the model is not producing,
  with a short crossfade on the handover: 0 silent seconds in the same test.
- **Voice effects now keep working while the AI warms up.** Previously "AI only" disabled
  pitch, drive, robot, echo, reverb and tone even when the AI was not yet converting, so
  those seconds were unprocessed. The effects are only handed over once the model is
  actually producing audio.
- **The window no longer names a preset that is not in effect.** Your saved parameters were
  restored, but the last preset's *name* was displayed regardless, so the UI could claim
  "Natural" while quite different settings were loaded. It now shows the preset only while
  the parameters still match it, and "Custom" otherwise.

### Added
- The AI card says "Warming up -- your live voice is passing through" while the model is
  catching up, instead of looking idle.
- The Home page points it out when the changer is on but nothing is actually altering the
  voice ("pick a preset, or turn a Quick tweak knob").

### Changed
- **"Replace my microphone everywhere"** (was "Use as the microphone for everything") now
  takes over completely. Every app recording right now is switched to the voice changer,
  including apps pinned to a specific device (Discord, OBS), not only apps set to "Default";
  apps that open while it is on are switched too; and every app plus your default microphone
  returns to its own device when you turn it off or quit. It stays an opt-in toggle and is
  remembered between runs.

## [0.2.4] - 2026-10-10

### Fixed
- **Apps that use the PulseAudio interface (Discord, most browsers, OBS) got silence from the
  virtual microphone.** It was a `pw_stream` source, which PulseAudio clients cannot negotiate
  with -- their capture stalls forever in "negotiating". The virtual microphone is now a proper
  driver node (PipeWire's `support.null-audio-sink` published as a virtual source), fed by the
  processed voice, so every app -- native PipeWire or PulseAudio -- captures it. Pick
  "Voice Changer Mic" in the app, or route it from the Mixer page.

## [0.2.3] - 2026-10-09

### Fixed
- **"Use as the microphone for everything" cut audio to every app** (Discord, routed
  streams, hear-myself all went silent). Our own capture stream follows the system default
  source, so making ourselves the default made WirePlumber route our capture onto our own
  output -- a feedback loop that dropped the real microphone. The capture is now pinned to a
  real microphone whenever we become the default, and restored when we stop.
- **"Hear myself" produced no sound.** The monitor stream was parked (connected inactive and
  auto-linked); activating it at runtime left the link stuck, so it never played. The monitor
  now stays connected and streaming, and the toggle gates the processed voice against silence,
  so it starts instantly.

## [0.2.2] - 2026-10-09

### Fixed
- Knobs did not follow a vertical drag (the start position was measured relative to the
  knob, the drag position absolutely, so the first move slammed the value to the minimum).
- Sliders could not be reset: iced's slider swallowed the press before the double-click or
  right-click wrapper saw it. Sliders are now drawn by the app with the same drag, click,
  wheel and reset behaviour as the knobs.
- The virtual microphone is held at 100% volume. Session managers restored a remembered
  level for it (one user's was at 16%), which made the processed voice nearly inaudible.
- Recording applications are listed by their process name when the stream is only called
  "WEBRTC VoiceEngine", so Discord and browsers show up as Discord, chrome, firefox.

## [0.2.1] - 2026-10-09

### Fixed
- Importing a `.pth` voice failed on a fresh machine with "No module named 'scipy'": the
  converter environment now installs scipy (RVC's model code imports it). Existing
  environments pick it up automatically on the next import.
- No hover help on the top bar; its controls explain themselves.

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
