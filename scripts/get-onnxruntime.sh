#!/usr/bin/env bash
# Downloads Microsoft's build of ONNX Runtime (the version the app is built
# against) into a directory: the CPU build as the fallback runtime, or with
# --cuda the GPU build for NVIDIA cards (needs the CUDA 12 runtime and
# cuDNN 9 installed on the system; AMD uses the distribution's onnxruntime-rocm).
#
#   scripts/get-onnxruntime.sh [dir]           CPU, default dir ~/.local/lib/voice-changer
#   scripts/get-onnxruntime.sh --cuda [dir]    CUDA, into <dir>/cuda
set -euo pipefail
VERSION="${ORT_VERSION:-1.22.0}"
CUDA=0
if [[ "${1:-}" == "--cuda" ]]; then CUDA=1; shift; fi
DIR="${1:-${XDG_DATA_HOME:-$HOME/.local}/lib/voice-changer}"
if [[ $CUDA == 1 ]]; then DIR="$DIR/cuda"; fi
case "$(uname -m)" in
    x86_64) ARCH=x64 ;;
    aarch64) ARCH=aarch64 ;;
    *) echo "unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac
if [[ $CUDA == 1 ]]; then NAME="onnxruntime-linux-$ARCH-gpu-$VERSION"; else NAME="onnxruntime-linux-$ARCH-$VERSION"; fi
URL="https://github.com/microsoft/onnxruntime/releases/download/v$VERSION/$NAME.tgz"
mkdir -p "$DIR"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
echo "downloading $URL"
curl -fL --progress-bar -o "$TMP/ort.tgz" "$URL"
tar -xzf "$TMP/ort.tgz" -C "$TMP"
cp "$TMP/$NAME/lib/libonnxruntime.so"* "$DIR/"
ln -sf "libonnxruntime.so.$VERSION" "$DIR/libonnxruntime.so" 2>/dev/null || true
if [[ $CUDA == 1 ]]; then
    echo "ONNX Runtime $VERSION (CUDA) installed in $DIR"
    echo "It needs the NVIDIA driver, the CUDA 12 runtime and cuDNN 9 on this machine; the app lists your GPU under Compute once they are present."
else
    echo "ONNX Runtime $VERSION installed in $DIR"
fi
