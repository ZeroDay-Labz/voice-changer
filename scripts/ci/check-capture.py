#!/usr/bin/env python3
"""Judge a capture of the virtual microphone.

    check-capture.py processed.wav [bypass.wav]

Fails when the processed capture is silent, contains a long silent gap, or
(when a bypass capture is given) does not differ from the untouched voice in
spectral centroid. Used by the CI smoke test; handy locally after
`parec -d <source> --file-format=wav out.wav`.
"""
import math
import struct
import sys
import wave


def read(path):
    with wave.open(path) as w:
        n, sw, rate = w.getnframes(), w.getsampwidth(), w.getframerate()
        raw = w.readframes(n)
    if sw == 2:
        data = [x / 32768.0 for x in struct.unpack("<%dh" % (len(raw) // 2), raw)]
    elif sw == 4:
        data = list(struct.unpack("<%df" % (len(raw) // 4), raw))
    else:
        raise SystemExit(f"{path}: unsupported sample width {sw}")
    return data, rate


def rms(x):
    return math.sqrt(sum(v * v for v in x) / max(1, len(x)))


def centroid(x, rate):
    """Spectral centroid in Hz over the voiced frames (needs numpy)."""
    import numpy as np

    a = np.asarray(x, dtype=np.float64)
    n = 4096
    freqs = np.fft.rfftfreq(n, 1.0 / rate)
    acc = 0.0
    weight = 0.0
    for start in range(0, len(a) - n, n // 2):
        frame = a[start : start + n]
        if np.sqrt(np.mean(frame * frame)) < 1e-3:
            continue
        mag = np.abs(np.fft.rfft(frame * np.hanning(n)))
        acc += float(np.sum(mag * freqs))
        weight += float(np.sum(mag))
    return acc / weight if weight > 0 else 0.0


def main():
    if len(sys.argv) < 2:
        raise SystemExit(__doc__)
    proc, rate = read(sys.argv[1])
    seconds = len(proc) / rate
    print(f"processed: {seconds:.1f} s at {rate} Hz, rms {rms(proc):.4f}")
    if seconds < 2.0:
        raise SystemExit("FAIL: capture shorter than 2 s (the virtual microphone produced nothing)")
    if rms(proc) < 0.005:
        raise SystemExit("FAIL: the captured audio is silent")
    gaps = 0
    for s in range(int(seconds)):
        chunk = proc[s * rate : (s + 1) * rate]
        level = rms(chunk)
        print(f"  second {s}: rms {level:.4f}")
        if level < 1e-4 and s >= 1:
            gaps += 1
    if gaps:
        raise SystemExit(f"FAIL: {gaps} silent second(s) in the capture")
    c_proc = centroid(proc, rate)
    print(f"processed centroid {c_proc:.0f} Hz")
    if len(sys.argv) > 2:
        byp, rate2 = read(sys.argv[2])
        c_byp = centroid(byp, rate2)
        print(f"bypass centroid {c_byp:.0f} Hz, rms {rms(byp):.4f}")
        if rms(byp) < 0.005:
            raise SystemExit("FAIL: the bypass capture is silent")
        if abs(c_proc - c_byp) < 0.15 * max(c_byp, 1.0):
            raise SystemExit("FAIL: the processed voice is not measurably different from bypass")
    print("OK")


if __name__ == "__main__":
    main()
