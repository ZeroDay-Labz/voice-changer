#!/usr/bin/env bash
# Convert an RVC voice model (.pth, as trained/shared by the RVC community)
# to the ONNX file this app loads.
#
#   scripts/convert-voice.sh MyVoice.pth            # -> ~/.local/share/voice-changer/models/voices/MyVoice.onnx
#   scripts/convert-voice.sh MyVoice.pth "Nice Name" # custom name
#
# First run creates a Python venv with CPU torch (~300 MB) under
# ~/.local/share/voice-changer/tools and fetches the RVC model code (MIT).
set -euo pipefail
PTH="${1:?usage: convert-voice.sh model.pth [name]}"
NAME="${2:-$(basename "${PTH%.*}")}"
HERE="$(cd "$(dirname "$0")" && pwd)"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}/voice-changer"
TOOLS="$DATA/tools"
VOICES="$DATA/models/voices"
VENV="$TOOLS/venv"
mkdir -p "$TOOLS" "$VOICES"

if [[ ! -x "$VENV/bin/python" ]]; then
    echo ">> creating venv (installs CPU torch, be patient)"
    python3 -m venv "$VENV"
    "$VENV/bin/pip" install --quiet --upgrade pip
fi
"$VENV/bin/python" -c "import torch, onnx, numpy" 2>/dev/null || \
    "$VENV/bin/pip" install --quiet torch --index-url https://download.pytorch.org/whl/cpu && \
    "$VENV/bin/pip" install --quiet onnx numpy onnxscript

OUT="$VOICES/$(echo "$NAME" | tr -c 'A-Za-z0-9._\n-' '_').onnx"
VC_TOOLS="$TOOLS" "$VENV/bin/python" "$HERE/rvc_export.py" "$(realpath "$PTH")" "$OUT"
echo ">> done. Pick \"$NAME\" under AI voice in the app (or: voice-changer ai \"$NAME\")."
