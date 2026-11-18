# Changelog

All notable changes to Voice Changer are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.6.2] - 2026-11-18

### Added
- **Copy problem report.** A button on every problem card and beside the log in Settings
  copies what went wrong together with the program version, the system, the graphics
  cards and their drivers, and the end of the log: paste it when asking for help.
- **First steps says why nothing is heard**: muted, or switched off, each with the button
  that undoes it.

### Changed
- **Problem cards are one plain sentence and a button that fixes it.** A graphics card
  that cannot run a voice offers "Use the processor"; the technical text is behind
  "Details".
- **"Fastest (graphics card)" is only offered in Delay when the AI really runs on a
  graphics card.** On Windows, and on computers without a usable card, it could only
  stutter.

### Fixed
- **A voice that fails on the graphics card now runs on the processor instead** of
  showing an error and retrying forever. Reported on Windows with a Radeon RX 7600M XT,
  where DirectML loads the voice and then fails on one of its steps. The voice is tested
  once on load; if it fails there and works on the processor, the processor is used until
  restart and the AI card says why. This has not been tried on a Windows computer.
- The error card no longer says "The AI engine is missing" for problems that are not
  that.

## [0.6.1] - 2026-11-18

### Fixed
- **The Flatpak is back.** 0.6.0 was published without it: a library added for the file
  dialogs needed a newer Rust than the Flatpak build environment has. It is pinned to a
  version that builds there.
- **`voice-changer get-runtime` works again.** Microsoft's ONNX Runtime repository
  disappeared from GitHub on 17 November 2026 and every download from it fails, in 0.6.0
  and earlier too. The runtime now comes from Microsoft's NuGet packages: the same 1.22.0
  libraries, byte for byte, and the download is now checked against a checksum.

## [0.6.0] - 2026-11-17

The interface was reworked for people who are not audio engineers, and there is a first
Windows version. Checked in a private, hardware-free PipeWire session and on the GitHub
build machines; see "Not tested" at the end.

### Added
- **Windows (preview).** A zip that runs from wherever it is unpacked. You pick a
  microphone and an output device and the changed voice plays on that output: no
  virtual audio cable, no driver, no change to your sound settings. Tray icon, global
  shortcuts, start with Windows, `voice-changer toggle` and the other commands, the CLAP
  plugin and AI voices are included. AI runs on the processor; the graphics card
  (DirectML) can be picked by hand, is marked experimental, and is left alone after a crash.
- **Pick the output.** A second picker in the top bar and in Settings chooses where the
  voice is heard: on Linux where Hear myself plays, on Windows where the changed voice
  goes. `voice-changer outputs` and `voice-changer output <name>` do the same.
- **Mute.** A button in the top bar, `Ctrl+U`, a global shortcut (`Ctrl+Shift+U`), a tray
  entry and `voice-changer mute`. It silences what you send with a short fade, also while
  the voice changer is off. It is never still on at the next start.
- **Real level meters.** Microphone and output bars with a dB scale, a shaded zone to aim
  for and a CLIP flag that stays for a few seconds.
- **Preset management.** An Edit mode on the preset grid renames, exports and deletes
  your presets (delete asks first); presets can be imported from a file or by dropping it
  on the window; **Update** stores changed settings back into the preset they came from;
  a built-in preset's name cannot be taken; an import never overwrites.
- **Add a voice from a file**, or drop it on the window.
- **First steps** card that ticks itself off from what is really happening: the
  microphone hears you, an output is chosen, a voice is picked. It can be shown again
  from Settings.
- **Settings:** buffer size (replaces `--quantum` advice), window size from 85% to 150%,
  a theme that follows the desktop, and a list of every shortcut.
- `voice-changer --ai-self-test` loads the AI engine and runs a tiny built-in model, and
  `doctor` uses it, so "the AI engine works" is checked rather than assumed.

### Changed
- **Plain words.** Labels and explanations no longer assume audio or AI vocabulary
  ("Keep my level steady", "Remove background noise", "Runs on", "Delay"). The AI card
  shows the few controls most people need; the rest is under **More settings**.
- **Problems come with their fix.** A missing base model, a missing AI engine or a voice
  file that is gone is explained in a sentence beside the button that deals with it;
  the technical message stays available. Error messages stay until dismissed. The
  window no longer tells you to run commands in a terminal.
- **The delay shown is the real one**: it includes the AI voice while one is running.
- The preset name changes to **Custom** as soon as a control no longer matches it.
- **The mouse wheel no longer changes a control the page scrolls past**; a control takes
  the wheel only when the pointer was resting on it. Hold Shift for fine steps.
- **NVIDIA needs only the driver.** `voice-changer get-runtime --cuda` now also downloads
  the NVIDIA libraries the AI needs on the graphics card (CUDA 12.8 and cuDNN 9.10, up
  to 1.7 GB, into `~/.local/lib/voice-changer/cuda`, no root) and the app loads them
  from there. Libraries the system already has are not downloaded again.
- The top bar drops its words and keeps its icons when the window is narrow. The
  "Send the voice to" panel is no longer repeated on the Settings page.

### Fixed
- **Ten controls had no hover explanation** (every Robot, Echo, Space and Tone knob, the
  voice-index switch): their help was filed under names the parameters no longer had. A
  test now fails when a control has none.
- **A dropout every half minute on some devices.** The output queue gave up cushion it
  needed, found out, took it back, and tried again. It now keeps what a device has shown
  it needs.
- **Light theme:** the knob pointer, the meter marks, the slider handle and coloured
  status text were close to invisible on a light background.
- **NVIDIA could not be set up on Fedora 42 and later.** The instructions named
  `cuda-toolkit-12` and `cudnn9` from NVIDIA's repository, which only carries CUDA 13
  for those releases, so the AI stayed on the processor.
- Picking "Hear myself" off and on again no longer moves it back to the default output.

### Not tested
- **Windows on real sound hardware.** The Windows build compiles, passes its unit tests
  and the engine self-test on a GitHub Windows machine, and loads and runs the AI engine
  there on the processor. Its audio path is the same code that is tested end to end on Linux (through ALSA
  into PipeWire, at 44.1, 48 and 96 kHz). But nobody has listened to it on Windows:
  latency, crackle, device hot-plug, the tray and the shortcuts are unverified.
- **DirectML (the graphics card on Windows) has never run.** The build machine has no
  usable graphics adapter: there the AI engine ran its test model on the processor, and
  asking for DirectML failed and fell back to the processor as designed.
- **A conversion on an NVIDIA GPU** (the download, the checksums and the loading of the
  libraries are verified, without an NVIDIA card).

## [0.5.0] - 2026-11-14

Measured on a Ryzen 9 5950X with a Radeon RX 6900 XT, on public test recordings (a
LibriSpeech sentence and ESC-50 coughs), not yet on the author's own voice.

### Added
- **Cough and burp blocker.** Coughs, burps, sneezes, throat clearing and hiccups are
  silenced instead of being converted into loud speech-like noise. Laughter is left to
  the voice. A small sound classifier (YAMNet, 15 MB, Apache-2.0, downloaded with the
  base models) listens to the last second before each block; it costs about 2 ms per
  look on the processor and adds no delay. On a sentence with a cough in it, between
  0 and 40 ms of the cough was audible over several runs (160 ms without the blocker);
  the same classifier never fired on the speech or the laughter clips, and did fire on
  some loud breathing.
- **Follow my loudness.** The converted voice now tracks the loudness of what goes in, so
  quiet breaths and mouth noises stay quiet and the noise gate works on the AI voice.
  The match between input and output loudness on a spoken sentence rose from 0.71 to
  0.88 (correlation of the 10 ms level envelopes) at the default 75%.
- **Protect consonants.** With an index, frames without pitch (breaths, s, t, sh) keep
  part of what was actually said instead of being pulled toward the index, as RVC's
  "protect" does. Default 67%.
- **Per-voice settings.** Pitch, index on/off and strength, breathiness, protection and
  loudness follow are remembered per voice and restored when it is selected.
- **Test bench** on the Voices page: record a phrase once (speech, cough, burp, laugh,
  breath) and replay it in a loop through any voice while tuning. A replayed clip goes
  to Hear myself only; the virtual microphone is silent meanwhile. `ai_score` (an
  example in `vc-core`) runs a recording through the real stage and prints delay,
  loudness tracking, pitch steadiness and how much of a marked sound got through.
- **Quality check for voices.** New imports are tried out once, and any installed voice
  can be checked: clipping, near-silent output, poor pitch tracking, a missing index and
  a low epoch count in the name are reported as a badge with the reasons.
- **voice-models.com links.** A voice page from that site can be pasted directly; the
  importer follows it to the Hugging Face download. Google Drive and MEGA links are
  refused with a plain explanation.
- The app learns your own typical speaking pitch while you talk and uses it to tell
  real pitch from the pitch model's mistakes.
- A hint on the AI card when noise removal is off while the AI voice is on.

### Changed
- **Pitch clean-up before the voice model.** Voiced blips shorter than 30 ms are dropped,
  pitch outside the speaker's plausible range is folded back by octaves or dropped, and
  the smoothing window is five frames instead of three. Counting pitch jumps in the
  output did not show a consistent gain on the test clips (the count varies more from
  run to run than between the two settings), so this is unproven by measurement.
- **The AI voice is about 0.3 s closer to real time.** The converted voice used to wait
  until two whole blocks were ready and then kept a block of audio in hand that was
  never needed: about 0.9 s from mouth to output where the app reported 0.5 s. It now
  starts with the first block and keeps a 50 ms cushion, which grows by itself when
  blocks come late and is given back afterwards. The reported delay is now the real one.
- **New "Fastest" AI speed** for GPUs (200 ms blocks); Auto starts there and backs off if
  the machine cannot keep up. Measured on a Radeon RX 6900 XT: 0.47 s from sound in to
  converted voice out (0.92 s before), at 60% load.
- **AI block sizes follow where the models really run.** A GPU build whose models run on
  the processor (no GPU, or one that could not be used) used the long GPU windows and
  worked the processor about a third harder than needed.
- The path to the virtual microphone no longer holds a block back (5 to 10 ms less).

### Fixed
- **"Hear myself" could make all other sound stutter.** Since 0.4.0 it put the output
  device on the microphone's clock, so a late microphone cycle was heard in everything
  playing, a call included. It is back on the output device's own clock; the slow drift
  between the two devices is absorbed by stretching a block by one sample now and then
  instead of dropping or repeating audio.

### Not included
- **Automatic pitch per voice.** Estimating the pitch a voice was trained at by scoring
  how well it tracks pitch at different shifts was tried and does not work (it put a
  male voice above a female one), so nothing sets pitch by itself. Per-voice settings
  cover the need: tune a voice once and it is remembered.

## [0.4.0] - 2026-11-13

### Fixed
- **Picking an AI voice aborted the whole app on machines using the CPU engine**, which is
  every NVIDIA and Intel machine by default (`std::vector<const OrtEpDevice*>::front()
  ... Assertion '!this->empty()' failed`, [#1](https://github.com/ZeroDay-Labz/voice-changer/issues/1)).
  The app asked ONNX Runtime 1.22 to choose a device by itself, which that version cannot
  do without a GPU provider. It no longer asks.
- **The launcher icon did nothing after installing a package** if the app had once been
  started from a build folder or an unpacked tarball: that run wrote a per-user launcher
  pointing at its own binary, which kept overriding the packaged one after the binary was
  gone. The leftover entry is now removed when a package provides the launcher.
- **The NVIDIA set-up could never work.** `scripts/get-onnxruntime.sh --cuda` copied the
  runtime but not the CUDA provider library next to it, and used the wrong folder when
  `XDG_DATA_HOME` was set. An install without the provider is no longer treated as a
  CUDA runtime.
- **A GPU that cannot be used is no longer reported as in use.** When the provider failed
  to load (CUDA or cuDNN missing, a driver problem) the status still named the GPU and
  the AI kept the small thread count meant for GPU runs, too few to keep up on the
  processor. It now switches to the processor with the right thread count and says why.
- **Quitting could leave "Voice Changer Mic" as the system's default microphone.** The
  restore was queued and the connection closed before it was sent. Quit now waits for the
  audio system to confirm. The previous default is also written to
  `~/.config/voice-changer/default-source.prev` while the app is the default, so a crash
  or a kill is undone on the next start.
- **The app can no longer be linked to its own virtual microphone.** Its nodes share a
  link group, which the session manager never links to itself. Unplugging the microphone
  while "Replace my microphone everywhere" is on now moves to another real microphone
  instead of "the default", which at that moment is the app itself.
- Denormal numbers are flushed to zero on the audio thread, so reverb and filter tails
  fading into silence cannot cause CPU spikes.

### Added
- **`voice-changer get-runtime [--cuda]`** downloads the AI engine for packaged installs,
  which do not have the `scripts` folder.
- **`voice-changer doctor` checks NVIDIA end to end**: the CUDA build of the runtime, the
  CUDA 12 libraries and cuDNN 9, with what to install for each missing piece.

### Changed
- **One clock for the whole audio path.** The microphone input, the processed-voice
  output, the virtual microphone and "Hear myself" now share one PipeWire scheduling
  group, so they run in the same graph cycle under the microphone's clock. Before, the
  microphone, the virtual microphone's timer and the headphones each ran on their own
  clock and the buffers between them slowly drifted apart until one ran dry or had to be
  cut short.
- **"Hear myself" is a stereo stream that exists only while it is on.** The voice is on
  both channels (both ears, front pair on surround outputs). When it is off there is no
  stream at all: it no longer keeps the headphones awake, sits in the volume mixer or
  ties the output device into the microphone's graph.
- **Smarter microphone channels.** The input is captured in stereo and folded down by the
  app. An interface or jack with the microphone on one channel is no longer halved, and a
  balanced cable whose two channels cancel each other is no longer silent. Mono and real
  stereo microphones sound as before.
- After a dropout the extra safety buffer is given back once things have been calm for
  half a minute (it used to stay for the rest of the session), and the voice fades back
  in after a gap instead of clicking.

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
