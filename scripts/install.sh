#!/usr/bin/env bash
# Builds release binaries and installs them for the current user:
#   ~/.local/bin/voice-changer          standalone app + CLI
#   ~/.clap/vc-plugin.clap              CLAP plugin for DAWs
#   ~/.local/share/applications/...     launcher, icons, AppStream metadata
# Options:
#   --cpu-only    build without the dynamic ONNX Runtime loader (no GPU support)
#   --autostart   also start it (minimized to the tray) at login
# The default build uses the system onnxruntime-rocm (AMD GPU) when present
# and otherwise a CPU ONNX Runtime, which this script downloads into
# ~/.local/lib/voice-changer if no distribution package provides one.
set -euo pipefail
cd "$(dirname "$0")/.."

APP_ID=io.github.zerodaylabz.VoiceChanger
FEATURES=()
AUTOSTART=0
for arg in "$@"; do
    case "$arg" in
        --cpu-only) FEATURES=(--no-default-features --features ai) ;;
        --autostart) AUTOSTART=1 ;;
        *) echo "unknown option: $arg" >&2; exit 2 ;;
    esac
done

cargo build --release -p vc-app "${FEATURES[@]}"
cargo xtask bundle vc-plugin --release

if [[ ${#FEATURES[@]} -eq 0 ]] && ! ls /usr/lib64/rocm/lib/libonnxruntime.so* /opt/rocm/lib/libonnxruntime.so* \
        /usr/lib64/libonnxruntime.so* /usr/lib/x86_64-linux-gnu/libonnxruntime.so* \
        "$HOME/.local/lib/voice-changer/libonnxruntime.so"* >/dev/null 2>&1; then
    echo "no ONNX Runtime found; fetching the CPU build (install onnxruntime-rocm for AMD GPU support)"
    scripts/get-onnxruntime.sh
fi

install -Dm755 target/release/voice-changer "$HOME/.local/bin/voice-changer"
install -Dm755 target/bundled/vc-plugin.clap "$HOME/.clap/vc-plugin.clap"
install -Dm644 "packaging/$APP_ID.desktop" "$HOME/.local/share/applications/$APP_ID.desktop"
install -Dm644 "packaging/$APP_ID.metainfo.xml" "$HOME/.local/share/metainfo/$APP_ID.metainfo.xml"
install -Dm644 "packaging/$APP_ID.svg" "$HOME/.local/share/icons/hicolor/scalable/apps/$APP_ID.svg"
for size in 16 32 48 64 128 256 512; do
    install -Dm644 "packaging/icons/$size.png" "$HOME/.local/share/icons/hicolor/${size}x${size}/apps/$APP_ID.png"
done

if [[ $AUTOSTART == 1 ]]; then
    sed 's/^Exec=voice-changer$/Exec=voice-changer --minimized/' "packaging/$APP_ID.desktop" \
        > "$HOME/.config/autostart/$APP_ID.desktop" 2>/dev/null \
        || { mkdir -p "$HOME/.config/autostart"; sed 's/^Exec=voice-changer$/Exec=voice-changer --minimized/' "packaging/$APP_ID.desktop" > "$HOME/.config/autostart/$APP_ID.desktop"; }
    echo "autostart enabled (~/.config/autostart/$APP_ID.desktop)"
fi

command -v update-desktop-database >/dev/null && update-desktop-database "$HOME/.local/share/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
echo "installed. Run: voice-changer   (toggle with: voice-changer toggle)"
