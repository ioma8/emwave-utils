#!/usr/bin/env python3
"""Passive emWave2 traffic logger. Opens the device non-exclusively and logs ALL
input reports (device->host) with timestamps. HID input reports are delivered to
every open client, and the device echoes host commands on the 'p' channel, so this
captures both directions during another app's session."""
import sys
import time
from emwave2 import EmWave2, report_payload

OUT = sys.argv[1] if len(sys.argv) > 1 else "/tmp/emwave2_capture.txt"
SECONDS = float(sys.argv[2]) if len(sys.argv) > 2 else 300

d = EmWave2()
print(f"capturing to {OUT} for {SECONDS}s (passive, no commands sent)")
t0 = time.time()
n = 0
with open(OUT, "w") as f:
    while time.time() - t0 < SECONDS:
        rep = d.read_report(1000)
        if rep is None:
            continue
        ts = time.time() - t0
        payload = report_payload(rep)
        if payload is None:
            continue
        line = f"{ts:8.3f} id=0x{rep[0]:02x} seq={rep[1]|(rep[2]<<8):04x} len={len(payload):3d} {payload.hex(' ')} '{payload.decode('ascii','replace')}'"
        print(line, flush=True)
        f.write(line + "\n")
        f.flush()
        n += 1
print(f"done, {n} reports -> {OUT}")
d.close()
