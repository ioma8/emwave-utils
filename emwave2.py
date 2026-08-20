#!/usr/bin/env python3
"""
emwave2.py — Python POC driver for the HeartMath emWave2 (USB HID).

Protocol decoded from:
  1. 328-byte HID report descriptor (IOKit registry, AppleUserUSBHostHIDDevice)
  2. radare2 RE of /Applications/emWave Pro.app (EmSensorSpace SDK, not stripped)

Wire format (validated from disassembly):
  Output report 'S' (0x53), 63 bytes via interrupt OUT EP 0x01:
      [0]   = 0x53 ('S')          report ID
      [1..2]= 0x00 0x00
      [3]   = strlen(command)
      [4..] = command text (e.g. "SR\r", "J+\r"), zero-padded to 63
  Input reports, 63 bytes via interrupt IN EP 0x81:
      [0]   = report ID ('u' IBI, 'p' PPG, 'e' events, 'F' flash, 'f' data)
      [1..2]= 16-bit sequence counter
      [3]   = payload length
      [4..] = payload
  'F' flash reports: [3..6]=32-bit address, [7]=len, [8..]=data
  Feature reports: 'r' (2B flags), '1' (13B info), 'I' (38B device ID), 't' (9B time)

Text protocol (device streams ASCII):
  Commands (via 'S' report): "J+\r" session begin, "J-\r" session end,
      "SR\r" start recording, "PD\r", "SB\r" session-blob download
  Records: "<I <ms>>" IBI, "<PU ...>", "<CO ACK>"/"<CO NAK>", "T=HH:MM:SS DD-MM-YY",
      key=value pairs, "<Sr...>"/"<SE...>" session records

Requires HID access (macOS 26: terminal app needs Input Monitoring grant, or run via
a sandboxed helper with com.apple.security.device.usb).
"""
from __future__ import annotations
import argparse
import ctypes
import re
import sys
import time

RAW_IBI_RE = re.compile(rb"<I (\d+)")
IBI_RE = re.compile(rb"<2 I=(\d+) R=([FT]) H=(\d+)")
STAT_RE = re.compile(rb"<1 L=(\d+) T=(\d+) S=(\d+) A=(\d+) E=(\d+)")

HIDAPI = "/opt/homebrew/opt/hidapi/lib/libhidapi.dylib"
VID, PID = 0x0E30, 0x0008
REPORT_LEN = 63  # 1 ID byte + 62 payload

def report_payload(report: bytes) -> bytes | None:
    """Return the declared payload, or None for a malformed report."""
    if len(report) < 4:
        return None
    length = report[3]
    if length > len(report) - 4:
        return None
    return report[4:4 + length]


def _hid_signature(lib: ctypes.CDLL) -> None:
    lib.hid_init.restype = ctypes.c_int
    lib.hid_init.argtypes = []
    lib.hid_exit.restype = ctypes.c_int
    lib.hid_exit.argtypes = []
    lib.hid_open.restype = ctypes.c_void_p
    lib.hid_open.argtypes = [ctypes.c_ushort, ctypes.c_ushort, ctypes.c_wchar_p]
    lib.hid_close.argtypes = [ctypes.c_void_p]
    lib.hid_write.restype = ctypes.c_int
    lib.hid_write.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]
    lib.hid_read_timeout.restype = ctypes.c_int
    lib.hid_read_timeout.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t, ctypes.c_int]
    lib.hid_get_feature_report.restype = ctypes.c_int
    lib.hid_get_feature_report.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]
    lib.hid_send_feature_report.restype = ctypes.c_int
    lib.hid_send_feature_report.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]
    lib.hid_error.restype = ctypes.c_wchar_p
    lib.hid_error.argtypes = [ctypes.c_void_p]


class EmWave2:
    """emWave2 HID client."""

    def __init__(self) -> None:
        self.lib = ctypes.CDLL(HIDAPI)
        _hid_signature(self.lib)
        if self.lib.hid_init() != 0:
            raise RuntimeError("hidapi init failed")
        self._dev = None
        self.open()

    # -- transport -----------------------------------------------------
    def open(self) -> None:
        dev = self.lib.hid_open(VID, PID, None)
        if not dev:
            raise RuntimeError(
                "emWave2 not found via HID. macOS 26 blocks HID for unpermissioned apps.\n"
                "  - run from a terminal app that has Input Monitoring (System Settings > Privacy > Input Monitoring)\n"
                "  - or wrap this script in a sandboxed helper app with com.apple.security.device.usb"
            )
        self._dev = ctypes.c_void_p(dev)

    def close(self) -> None:
        if self._dev:
            self.lib.hid_close(self._dev)
            self._dev = None
        self.lib.hid_exit()

    def _err(self) -> str:
        return self.lib.hid_error(self._dev) or "?"

    def send_command(self, cmd: bytes) -> None:
        """Send a text command in an 'S' output report."""
        buf = bytearray(REPORT_LEN)
        buf[0] = 0x53
        buf[3] = len(cmd) & 0xFF
        buf[4:4 + len(cmd)] = cmd
        n = self.lib.hid_write(self._dev, bytes(buf), REPORT_LEN)
        if n < 0:
            raise RuntimeError(f"hid_write failed: {self._err()}")

    def read_report(self, timeout_ms: int = 1000) -> bytes | None:
        """Read one input report (63 bytes) or None on timeout."""
        buf = ctypes.create_string_buffer(REPORT_LEN)
        n = self.lib.hid_read_timeout(self._dev, buf, REPORT_LEN, timeout_ms)
        if n < 0:
            raise RuntimeError(f"hid_read failed: {self._err()}")
        if n == 0:
            return None
        return buf.raw[:n]

    def get_feature(self, report_id: int, size: int) -> bytes:
        """GET_REPORT(feature). size = payload bytes (excludes ID)."""
        buf = ctypes.create_string_buffer(size + 1)
        buf[0] = chr(report_id).encode()
        n = self.lib.hid_get_feature_report(self._dev, buf, size + 1)
        if n < 0:
            raise RuntimeError(f"get_feature 0x{report_id:02x} failed: {self._err()}")
        return buf.raw[:n]

    def set_feature(self, report_id: int, payload: bytes) -> None:
        buf = bytes([report_id]) + payload
        n = self.lib.hid_send_feature_report(self._dev, buf, len(buf))
        if n < 0:
            raise RuntimeError(f"set_feature 0x{report_id:02x} failed: {self._err()}")

    # -- protocol ------------------------------------------------------
    def get_device_info(self) -> dict:
        """Feature 'I' (38B) device ID + feature 't' (9B) time."""
        info = {}
        try:
            rid = self.get_feature(0x49, 38)   # 'I'
            info["device_id_raw"] = rid.hex(" ")
        except RuntimeError as e:
            info["device_id_err"] = str(e)
        try:
            t = self.get_feature(0x74, 9)      # 't'
            info["time_raw"] = t.hex(" ")
        except RuntimeError as e:
            info["time_err"] = str(e)
        return info

    def session_flag(self, on: bool) -> None:
        """Feature 'r': byte[1] bit0 = session in progress."""
        cur = self.get_feature(0x72, 2)        # 'r'
        b = cur[1] if len(cur) > 1 else 0
        b = (b & 0x80) | (0x01 if on else 0x00)
        self.set_feature(0x72, bytes([b & 0xFF]))

    def start_session(self) -> None:
        self.session_flag(True)
        self.send_command(b"J-\r")             # end any previous session
        self.send_command(b"SR\r")             # start recording

    def stop_session(self) -> None:
        # App's StopDevice: send "PD\r" (pause device), then close. The 'r'
        # feature flag is set for command mode but never explicitly cleared.
        try:
            self.send_command(b"PD\r")
        except RuntimeError:
            pass

    def iter_reports(self, report_id: int | None, seconds: float, timeout_ms: int = 1000):
        end = time.time() + seconds
        while time.time() < end:
            rep = self.read_report(timeout_ms)
            if rep is None or report_payload(rep) is None:
                continue
            if report_id is not None and rep[0] != report_id:
                continue
            yield rep


def cmd_info(d: EmWave2, _args) -> int:
    print(d.get_device_info())
    return 0


def cmd_ibi(d: EmWave2, args) -> int:
    print(f"[*] starting IBI session ({args.seconds}s)...")
    d.start_session()
    def emit(line: bytes) -> None:
        nonlocal n
        if b"<SESSIONSTART>" in line:
            print("[SESSION START]")
        for m in RAW_IBI_RE.finditer(line):
            print(f"RAW IBI={int(m.group(1)):4d}ms")
            n += 1
        for m in IBI_RE.finditer(line):
            ibi, flag, hr = int(m.group(1)), m.group(2).decode(), int(m.group(3))
            print(f"IBI={ibi:4d}ms artifact={'yes' if flag == 'T' else 'no '} HR={hr} bpm")
            n += 1
        for m in STAT_RE.finditer(line):
            l, t, s, a, e = (int(m.group(i)) for i in range(1, 6))
            print(f"STATUS L={l} T={t} S={s} A={a} E={e}")

    try:
        n = 0
        buf = b""
        for rep in d.iter_reports(0x75, args.seconds):   # 'u'
            payload = report_payload(rep)
            if payload is None:
                continue
            buf += payload
            while b"\r" in buf:                          # records end with \r
                line, buf = buf.split(b"\r", 1)
                if line.strip():
                    emit(line)
        if buf.strip():
            emit(buf)
    except RuntimeError as error:
        print(f"[!] stream error: {error}", file=sys.stderr)
        return 1
    finally:
        d.stop_session()
    print(f"[*] {n} IBI records")
    return 0


def cmd_ppg(d: EmWave2, args) -> int:
    print(f"[*] capturing reports ({args.seconds}s)...")
    print("[!] note: emWave2 USB streams IBI/status text only (validated live).")
    print("[!] 'p' reports are the command-echo channel; raw pulse samples are not")
    print("[!] streamed. Raw pulse (<PU ...>) appears only in stored session blobs.")
    d.start_session()
    try:
        n = 0
        for rep in d.iter_reports(None, args.seconds):
            payload = report_payload(rep)
            if payload is None:
                continue
            if rep[0] == 0x70 and payload:
                text = payload.decode("ascii", "replace")
                print(f"echo({len(payload)}B): {text!r}")
                n += 1
            elif rep[0] in (0x75, 0x65, 0x66) and payload:
                text = payload.decode("ascii", "replace")
                print(f"id=0x{rep[0]:02x} len={len(payload)}: {text!r}")
                n += 1
    except RuntimeError as error:
        print(f"[!] stream error: {error}", file=sys.stderr)
        return 1
    finally:
        d.stop_session()
    print(f"[*] {n} reports captured")
    return 0


def cmd_events(d: EmWave2, args) -> int:
    print(f"[*] listening for event/error reports ({args.seconds}s)...")
    d.start_session()
    try:
        for rep in d.iter_reports(None, args.seconds):
            rid = rep[0]
            if rid in (0x65, 0x66, 0x53):     # 'e' events, 'f' data, 'S' ACK echo
                payload = report_payload(rep)
                if payload is None:
                    continue
                print(f"id=0x{rid:02x} seq={rep[1] | (rep[2] << 8):04x}: {payload.hex(' ')} "
                      f"'{payload.decode('ascii', 'replace')}'")
    finally:
        d.stop_session()
    return 0


def cmd_sessions(d: EmWave2, args) -> int:
    """Download stored sessions (validated live 2026-08-19).

    Sequence: feature 'I' read, feature 't' read, then "SB\\r".
    Device responds with:
      - 'F' (0x46) reports: binary blob blocks (first = blob header)
      - 'f' (0x66) reports: text session records (<Sr T=..>, <Ss T=..>, <PU ..>,
        <SESSIONSTART>, IBI records, CO ACK)
    Captures both; saves the raw 'F' stream and the reassembled text log.
    """
    print("[*] device info (app sync order: I, t, then SB)...")
    d.get_feature(0x49, 38)                       # 'I' device id
    d.get_feature(0x74, 9)                        # 't' time
    d.session_flag(True)
    print("[*] requesting stored sessions (SB\\r)...")
    d.send_command(b"SB\r")
    fblocks = bytearray()
    text = bytearray()
    end = time.time() + args.seconds
    n = 0
    while time.time() < end:
        rep = d.read_report(2000)
        if rep is None:
            continue
        payload = report_payload(rep)
        if payload is None:
            continue
        rid = rep[0]
        if rid == 0x46:
            fblocks += payload
            n += 1
        elif rid == 0x66:
            text += payload
            if b"\r" in payload or n == 0:
                sys.stdout.write(payload.decode("ascii", "replace"))
                sys.stdout.flush()
    out = args.out or "emwave2_sessions.bin"
    with open(out, "wb") as f:
        f.write(bytes(fblocks))
    tpath = out + ".txt"
    with open(tpath, "wb") as f:
        f.write(bytes(text))
    print(f"\n[*] {n} F blocks ({len(fblocks)} bytes) -> {out}")
    print(f"[*] session text ({len(text)} bytes) -> {tpath}")
    return 0


def cmd_firmware(d: EmWave2, args) -> int:
    """Experimental firmware dump. readRawFlash is disabled in the Mac app (stub 0x6c),
    so the flash-read command must be probed live. This is best-effort."""
    print("[!] firmware dump is EXPERIMENTAL (Mac app's readRawFlash is a disabled stub).")
    print("[*] probing feature reports for flash/bootloader info...")
    for rid, size, name in [(0x31, 13, "1/info"), (0x49, 38, "I/devid"), (0x62, 8, "b/boot"),
                            (0x72, 2, "r/flags"), (0x77, 63, "w/write"), (0x04, 62, "bl-data")]:
        try:
            data = d.get_feature(rid, size)
            print(f"  feature 0x{rid:02x} ({name}) [{len(data)}B]: {data.hex(' ')}")
        except RuntimeError as e:
            print(f"  feature 0x{rid:02x} ({name}): {e}")
    print("[*] listening for 'F' flash reports (30s) in case device streams...")
    blob = bytearray()
    end = time.time() + 30
    n = 0
    while time.time() < end:
        rep = d.read_report(2000)
        payload = report_payload(rep) if rep is not None else None
        if payload is None or rep[0] != 0x46 or len(rep) < 8:
            continue
        addr = int.from_bytes(rep[3:7], "little")
        length = rep[7]
        data = rep[8:8 + length]
        if len(blob) < addr + length:
            blob.extend(b"\x00" * (addr + length - len(blob)))
        blob[addr:addr + length] = data
        n += 1
        print(f"  F block addr=0x{addr:06x} len={length}")
    if args.out and blob:
        with open(args.out, "wb") as f:
            f.write(blob)
        print(f"[*] wrote {len(blob)} bytes -> {args.out}")
    print(f"[*] done ({n} F blocks)")
    return 0


def cmd_raw(d: EmWave2, args) -> int:
    """Dump all raw input reports (validation aid)."""
    print(f"[*] raw capture {args.seconds}s...")
    d.start_session()
    try:
        for rep in d.iter_reports(None, args.seconds):
            payload = report_payload(rep)
            print(f"id=0x{rep[0]:02x} seq={rep[1] | (rep[2] << 8):04x} "
                  f"len={len(payload):3d} {payload.hex(' ')} "
                  f"'{payload.decode('ascii', 'replace')}'")
    except RuntimeError as error:
        print(f"[!] stream error: {error}", file=sys.stderr)
        return 1
    finally:
        d.stop_session()
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description="emWave2 POC driver")
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("info").set_defaults(fn=cmd_info)
    p = sub.add_parser("raw"); p.add_argument("--seconds", type=float, default=10.0); p.set_defaults(fn=cmd_raw)
    p = sub.add_parser("ibi"); p.add_argument("--seconds", type=float, default=10.0); p.set_defaults(fn=cmd_ibi)
    p = sub.add_parser("ppg"); p.add_argument("--seconds", type=float, default=10.0); p.set_defaults(fn=cmd_ppg)
    p = sub.add_parser("events"); p.add_argument("--seconds", type=float, default=10.0); p.set_defaults(fn=cmd_events)
    p = sub.add_parser("sessions"); p.add_argument("--seconds", type=float, default=60.0)
    p.add_argument("--out", default=None); p.set_defaults(fn=cmd_sessions)
    p = sub.add_parser("firmware"); p.add_argument("--out", default=None); p.set_defaults(fn=cmd_firmware)
    args = ap.parse_args()

    d = EmWave2()
    try:
        return args.fn(d, args)
    finally:
        d.close()


if __name__ == "__main__":
    sys.exit(main())
