# Flatpak (draft)

The manifest builds the app against the freedesktop 25.08 runtime. It is not
on Flathub yet; the RPM, .deb and tarball from the GitHub release are the
supported installs for now.

Generate the offline cargo sources after every `Cargo.lock` change:

```sh
curl -LO https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
python3 flatpak-cargo-generator.py Cargo.lock -o packaging/flatpak/cargo-sources.json
```

Then build and install for your user:

```sh
flatpak install flathub org.freedesktop.Sdk//25.08 org.freedesktop.Sdk.Extension.rust-stable//25.08 org.freedesktop.Sdk.Extension.llvm20//25.08
flatpak-builder --user --install --force-clean build-dir packaging/flatpak/io.github.zerodaylabz.VoiceChanger.yml
```

Known limits inside the sandbox: the `.pth` → ONNX converter needs Python
and PyTorch, which the sandbox does not have. Convert on the host with
`scripts/convert-voice.sh` and drop the `.onnx` into
`~/.var/app/io.github.zerodaylabz.VoiceChanger/data/voice-changer/models/voices/`.
GPU (ROCm) inference needs the host's `onnxruntime-rocm`, which the Flatpak
cannot see; use the native package for that.
