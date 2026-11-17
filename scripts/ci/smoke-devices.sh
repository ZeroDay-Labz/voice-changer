#!/usr/bin/env bash
# End-to-end test of the pick-your-devices audio backend (the one Windows
# uses), on Linux: a private PipeWire session with a null microphone and
# null speakers, reached through ALSA the way any plain audio program would.
# The app plays a built-in synthetic voice to the speakers; what arrives
# there is recorded and checked for having been changed.
#
#   scripts/ci/smoke-devices.sh path/to/voice-changer   (built with --features backend-cpal)
#
# Needs: pipewire, wireplumber, pipewire-pulse, pipewire-alsa, pactl, pw-cli,
# dbus-launch, python3 with numpy.
set -euo pipefail
APP="${1:?path to the voice-changer binary}"
APP="$(readlink -f "$APP")"
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${SMOKE_OUT:-/tmp/vc-smoke-devices}"
mkdir -p "$OUT"

export XDG_RUNTIME_DIR="$OUT/runtime"
export XDG_CONFIG_HOME="$OUT/config"
export XDG_DATA_HOME="$OUT/data"
mkdir -p "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME/voice-changer" "$XDG_DATA_HOME"
chmod 700 "$XDG_RUNTIME_DIR"
echo '(first_run: false)' > "$XDG_CONFIG_HOME/voice-changer/config.ron"
if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
    eval "$(dbus-launch --sh-syntax)"
    export DBUS_SESSION_BUS_ADDRESS
fi

PIDS=()
cleanup() {
    for p in "${PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done
    [ -n "${DBUS_SESSION_BUS_PID:-}" ] && kill "$DBUS_SESSION_BUS_PID" 2>/dev/null || true
}
trap cleanup EXIT

wait_for() { # name, command...
    local name="$1"; shift
    for _ in $(seq 1 40); do
        if "$@" >/dev/null 2>&1; then return 0; fi
        sleep 0.25
    done
    echo "FAIL: $name did not come up" >&2
    return 1
}

echo "== starting a private PipeWire session in $XDG_RUNTIME_DIR"
pipewire > "$OUT/pipewire.log" 2>&1 &
PIDS+=($!)
wait_for "pipewire" pw-cli info 0
wireplumber > "$OUT/wireplumber.log" 2>&1 &
PIDS+=($!)
pipewire-pulse > "$OUT/pipewire-pulse.log" 2>&1 &
PIDS+=($!)
wait_for "pipewire-pulse" pactl info
wait_for "wireplumber (default metadata)" sh -c "pw-cli ls Metadata | grep -q 'metadata.name = \"default\"'"

pw-cli create-node adapter '{ factory.name=support.null-audio-sink node.name=ci-speakers node.description="CI speakers" media.class=Audio/Sink audio.position=[FL,FR] object.linger=true }' >/dev/null
pw-cli create-node adapter '{ factory.name=support.null-audio-sink node.name=ci-microphone node.description="CI microphone" media.class=Audio/Source/Virtual audio.position=[MONO] object.linger=true }' >/dev/null
sleep 1
pactl set-default-sink ci-speakers
pactl set-default-source ci-microphone

run_capture() { # label, output.wav, app args...
    local label="$1" wav="$2"; shift 2
    echo "== $label"
    "$APP" --no-dbus --headless --input-wav synth "$@" > "$OUT/app-$label.log" 2>&1 &
    local app=$!
    sleep 4
    timeout 6 parec -d ci-speakers.monitor --file-format=wav --channels=1 --rate=48000 "$wav" || true
    kill -INT "$app" 2>/dev/null || true
    wait "$app" 2>/dev/null || true
    grep -vE "ort::" "$OUT/app-$label.log" | tail -6 | sed 's/^/   /'
    if ! grep -q "audio backend" "$OUT/app-$label.log"; then
        echo "FAIL: this binary was not built with --features backend-cpal" >&2
        exit 1
    fi
}
run_capture robot "$OUT/robot.wav" --on --preset Robot
run_capture bypass "$OUT/bypass.wav" --bypass
# The same again through the sample-rate converters (devices at 44.1 kHz).
VC_TEST_DEVICE_RATE=44100 run_capture robot441 "$OUT/robot441.wav" --on --preset Robot

echo "== judging the captures"
python3 "$HERE/check-capture.py" "$OUT/robot.wav" "$OUT/bypass.wav"
python3 "$HERE/check-capture.py" "$OUT/robot441.wav" "$OUT/bypass.wav"
