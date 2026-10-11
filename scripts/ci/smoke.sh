#!/usr/bin/env bash
# End-to-end smoke test on a machine without sound hardware (CI runner,
# container): start a private PipeWire session (daemon, WirePlumber, the
# PulseAudio bridge), give it a null microphone and speakers, run the voice
# changer on a built-in synthetic voice, record its virtual microphone
# through the PulseAudio API the way Discord and browsers do, and check that
# the voice came out changed.
#
#   scripts/ci/smoke.sh path/to/voice-changer
#
# Needs: pipewire, wireplumber, pipewire-pulse, pactl (pulseaudio-utils),
# pw-cli, dbus-launch (dbus-x11 or dbus-daemon), python3 with numpy.
set -euo pipefail
APP="${1:?path to the voice-changer binary}"
APP="$(readlink -f "$APP")"
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${SMOKE_OUT:-/tmp/vc-smoke}"
mkdir -p "$OUT"

export XDG_RUNTIME_DIR="$OUT/runtime"
export XDG_CONFIG_HOME="$OUT/config"
export XDG_DATA_HOME="$OUT/data"
mkdir -p "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME"
chmod 700 "$XDG_RUNTIME_DIR"
# Without a session bus PipeWire's realtime module and WirePlumber complain,
# and on some images refuse to come up at all.
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
pactl info | grep -E "Server Name|Server Version"

# A runner has no sound card: null devices stand in for speakers and a mic,
# created the same way the app creates its virtual microphone.
pw-cli create-node adapter '{ factory.name=support.null-audio-sink node.name=ci-speakers node.description="CI speakers" media.class=Audio/Sink audio.position=[FL,FR] object.linger=true }' >/dev/null
pw-cli create-node adapter '{ factory.name=support.null-audio-sink node.name=ci-microphone node.description="CI microphone" media.class=Audio/Source/Virtual audio.position=[MONO] object.linger=true }' >/dev/null
sleep 1
pw-cli ls Node | grep -E "node.name" | sed 's/^/   /'

echo "== doctor"
"$APP" doctor || true

run_capture() { # label, output.wav, app args...
    local label="$1" wav="$2"; shift 2
    echo "== $label"
    "$APP" --no-dbus --headless --input-wav synth --source-name "vc_$label" "$@" > "$OUT/app-$label.log" 2>&1 &
    local app=$!
    wait_for "virtual mic vc_$label" sh -c "pactl list sources short | grep -q vc_$label"
    sleep 2
    timeout 6 parec -d "vc_$label" --file-format=wav --channels=1 --rate=48000 "$wav" || true
    kill -INT "$app" 2>/dev/null || true
    wait "$app" 2>/dev/null || true
    grep -vE "ort::" "$OUT/app-$label.log" | tail -8 | sed 's/^/   /'
}
run_capture robot "$OUT/robot.wav" --on --preset Robot
run_capture bypass "$OUT/bypass.wav" --bypass

echo "== judging the captures"
python3 "$HERE/check-capture.py" "$OUT/robot.wav" "$OUT/bypass.wav"
