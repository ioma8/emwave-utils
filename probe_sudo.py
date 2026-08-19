#!/usr/bin/env python3
"""Root-level emWave2 probe: interface-level USB access bypasses macOS ACLs.
Run:  sudo /opt/homebrew/bin/python3 probe_sudo.py [seconds]
"""
import sys
import time
import usb.core

DEV = (0x0e30, 0x0008)
SECONDS = int(sys.argv[1]) if len(sys.argv) > 1 else 6

dev = usb.core.find(idVendor=DEV[0], idProduct=DEV[1])
if dev is None:
    print("device not found")
    sys.exit(1)

# 1) HID report descriptor (interface-level, needs root)
try:
    rd = dev.ctrl_transfer(0x81, 6, 0x2200, 0, 328, timeout=1000)
    print("HID_REPORT_DESC(%d): %s" % (len(rd), " ".join("%02x" % b for b in rd)))
except Exception as e:
    print("HID_REPORT_DESC: FAIL %s" % e)

# 2) probe feature reports 0..15 for bootloader/debug hooks
for rid in range(16):
    try:
        fr = dev.ctrl_transfer(0xA1, 1, rid << 8, 0, 64, timeout=500)
        print("FEATURE[%d](%d): %s" % (rid, len(fr), " ".join("%02x" % b for b in fr)))
    except Exception as e:
        print("FEATURE[%d]: FAIL %s" % (rid, e))

# 3) claim interface, stream interrupt IN
try:
    dev.set_configuration()
    import usb.util
    try:
        usb.util.claim_interface(dev, 0)
        print("INTERFACE: claimed")
    except Exception as e:
        print("INTERFACE claim: %s" % e)
    end = time.time() + SECONDS
    n = 0
    while time.time() < end:
        try:
            data = dev.read(0x81, 64, timeout=1000)
            n += 1
            print("IN(%d): %s" % (len(data), " ".join("%02x" % b for b in data)))
        except usb.core.USBError as e:
            if e.errno == 110:  # timeout
                print("IN: timeout (no data)")
            else:
                print("IN: FAIL %s" % e)
                break
    print("STREAM_DONE reports=%d" % n)
except Exception as e:
    print("STREAM: FAIL %s" % e)
