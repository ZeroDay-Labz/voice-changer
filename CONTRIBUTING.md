# Contributing

Thanks for taking a look. Small, focused pull requests are easiest to review.

## Building

```sh
sudo dnf install pipewire-devel clang gcc-c++ libxkbcommon-devel wayland-devel   # Fedora
sudo apt install libpipewire-0.3-dev libclang-dev pkg-config libxkbcommon-dev libwayland-dev build-essential   # Debian/Ubuntu
cargo build                      # the app (debug builds are optimised enough to use)
cargo test --workspace
cargo clippy --workspace --all-targets
cargo xtask bundle vc-plugin --release   # the CLAP plugin → target/bundled/
```

## Ground rules

- `vc-dsp` runs on the audio thread: no allocation, locks or I/O in `process()`.
  Add a unit test for any new DSP behaviour (see the click and transparency tests).
- The UI lives in `vc-gui` and must keep working in both hosts (app and plugin). The
  `Host` trait is the only way the UI reaches host features.
- Run `cargo fmt`, keep clippy clean, and update `CHANGELOG.md` under "Unreleased".
- Screenshots for the README come from the app itself:
  `voice-changer --screenshot docs/screenshots/<page>.png --page <page> --on`.
- Do not add voice models to the repository. Link to them and respect their licences.

## Reporting problems

Please include your distribution, PipeWire version (`pw-cli info 0 | head`), the output of
`voice-changer --help`, and the log lines from Settings → Log or `RUST_LOG=debug`.
