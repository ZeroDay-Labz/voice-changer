# Flatpak

The release workflow builds `VoiceChanger.flatpak` from this manifest (freedesktop
24.08 runtime) and attaches it to every GitHub release:

```sh
flatpak install --user ./VoiceChanger.flatpak
```

It is not on Flathub yet. Inside the sandbox the AI runs on the CPU (the host's
ROCm/CUDA runtime is not visible), so on an AMD or NVIDIA machine the native
package is the better choice.

Generate the offline cargo sources after every `Cargo.lock` change:

```sh
curl -LO https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
python3 flatpak-cargo-generator.py Cargo.lock -o packaging/flatpak/cargo-sources.json
```

Then build and install for your user:

```sh
flatpak install flathub org.freedesktop.Sdk//24.08 org.freedesktop.Sdk.Extension.rust-stable//24.08 org.freedesktop.Sdk.Extension.llvm18//24.08
flatpak-builder --user --install --force-clean build-dir packaging/flatpak/io.github.zerodaylabz.VoiceChanger.yml
```

Known limits inside the sandbox: the `.pth` → ONNX converter needs Python
and PyTorch, which the sandbox does not have. Convert on the host with
`scripts/convert-voice.sh` and drop the `.onnx` into
`~/.var/app/io.github.zerodaylabz.VoiceChanger/data/voice-changer/models/voices/`.
GPU (ROCm) inference needs the host's `onnxruntime-rocm`, which the Flatpak
cannot see; use the native package for that.
