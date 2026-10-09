#!/usr/bin/env bash
# Downloads Microsoft's CPU build of ONNX Runtime (the version the app is
# built against) into a directory, as the fallback runtime for machines
# without the distribution's onnxruntime / onnxruntime-rocm package.
#
#   scripts/get-onnxruntime.sh [dir]     default: ~/.local/lib/voice-changer
set -euo pipefail
VERSION="${ORT_VERSION:-1.22.0}"
DIR="${1:-${XDG_DATA_HOME:-$HOME/.local}/lib/voice-changer}"
case "$(uname -m)" in
    x86_64) ARCH=x64 ;;
    aarch64) ARCH=aarch64 ;;
    *) echo "unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac
NAME="onnxruntime-linux-$ARCH-$VERSION"
URL="https://github.com/microsoft/onnxruntime/releases/download/v$VERSION/$NAME.tgz"
mkdir -p "$DIR"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
echo "downloading $URL"
curl -fL --progress-bar -o "$TMP/ort.tgz" "$URL"
tar -xzf "$TMP/ort.tgz" -C "$TMP"
cp "$TMP/$NAME/lib/libonnxruntime.so"* "$DIR/"
ln -sf "libonnxruntime.so.$VERSION" "$DIR/libonnxruntime.so" 2>/dev/null || true
echo "ONNX Runtime $VERSION installed in $DIR"
