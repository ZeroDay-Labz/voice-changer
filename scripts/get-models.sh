#!/usr/bin/env bash
# Downloads the AI base models (MIT licensed, ~740 MB) into the folder the
# app reads: ~/.local/share/voice-changer/models
# Voice models (*.onnx exported from RVC) go into .../models/voices/
set -euo pipefail
DIR="${XDG_DATA_HOME:-$HOME/.local/share}/voice-changer/models"
mkdir -p "$DIR/voices"
BASE="https://huggingface.co/TigreGotico/voiceclonnx-rvc/resolve/main"
for f in contentvec_768l12.onnx rmvpe.onnx; do
    if [[ -s "$DIR/$f" ]]; then
        echo "have $f"
    else
        echo "downloading $f ..."
        curl -L --progress-bar -o "$DIR/$f.part" "$BASE/$f" && mv "$DIR/$f.part" "$DIR/$f"
    fi
done
echo
echo "Base models are in $DIR"
echo "Put RVC voice models (.onnx, exported with vconnx or VCClient) in $DIR/voices/"
echo "then pick them under 'AI voice' in the app."
