#!/usr/bin/env python3
"""emwave_tui.py — live HR / HRV / baroreflex resonance dashboard for emWave2.

Based on Task Force (1996), Shaffer & Ginsberg (2017), and Shaffer & Meehan (2020)
validated standards (see HRV_COHERENCE_RESEARCH.md).

Displays:
  - Live HR & IBI (from qualified <2 I=.. R=.. H=..> records)
  - 6.0 bpm visual breath pacer (4s inhale / 6s exhale) to guide resonance
  - Time-domain HRV: SDNN, RMSSD, pNN50, Mean RR
  - Frequency-domain: LF (0.04-0.15 Hz), HF (0.15-0.40 Hz), LF_nu, Peak Frequency
  - Baroreflex Resonance Score & Physiological State (RESONANT, PARASYMPATHETIC, BALANCED, STRESS)
"""
from __future__ import annotations

import argparse
import curses
import math
import re
import sys
import time

import numpy as np

from emwave2 import EmWave2, IBI_RE, STAT_RE

SESSIONSTART_RE = re.compile(rb"<SESSIONSTART>")


# --------------------------------------------------------------------------
# Validated HRV & Resonance Math (Task Force 1996, Shaffer 2017/2020)
# --------------------------------------------------------------------------

def hrv_metrics(ibis: list[tuple[float, float]]) -> dict | None:
    """Calculates standard time-domain metrics for clean NN intervals.
    ibis: [(beat_time_sec, ibi_ms), ...]
    """
    if len(ibis) < 4:
        return None
    nn = np.array([m for _, m in ibis], dtype=float)
    diffs = np.diff(nn)
    if len(diffs) < 1:
        return None

    mean_rr = float(np.mean(nn))
    sdnn = float(np.std(nn, ddof=1)) if len(nn) > 1 else 0.0
    rmssd = float(np.sqrt(np.mean(diffs ** 2)))
    pnn50 = float(np.sum(np.abs(diffs) > 50.0) / len(diffs) * 100.0)
    hr_max_min = float(np.max(nn) - np.min(nn))

    return {
        "n": len(nn),
        "sdnn": sdnn,
        "rmssd": rmssd,
        "pnn50": pnn50,
        "hr_max_min": hr_max_min,
        "mean_rr": mean_rr,
        "mean_hr": 60000.0 / mean_rr if mean_rr > 0 else 0.0,
    }


def resonance_analysis(ibis: list[tuple[float, float]]) -> dict | None:
    """Calculates frequency-domain metrics and baroreflex resonance alignment.
    Reconstructs time series at 4 Hz, detrends, applies Hanning window,
    and analyzes LF (0.04-0.15 Hz) vs HF (0.15-0.40 Hz) bands.
    """
    if len(ibis) < 16:
        return None

    t = np.array([ts for ts, _ in ibis], dtype=float)
    y = np.array([m for _, m in ibis], dtype=float)
    span = t[-1] - t[0]
    if span < 12.0:
        return None

    fs = 4.0
    n_pts = int(span * fs)
    if n_pts < 16:
        return None

    grid = np.linspace(t[0], t[-1], n_pts)
    yi = np.interp(grid, t, y)
    yi = yi - np.mean(yi)  # detrend mean

    win = np.hanning(len(yi))
    fft_vals = np.fft.rfft(yi * win)
    spec = np.abs(fft_vals) ** 2
    freqs = np.fft.rfftfreq(len(yi), 1.0 / fs)

    def band_power(f0: float, f1: float) -> float:
        mask = (freqs >= f0) & (freqs < f1)
        return float(np.sum(spec[mask])) if np.any(mask) else 0.0

    vlf = band_power(0.0033, 0.04)
    lf = band_power(0.04, 0.15)
    hf = band_power(0.15, 0.40)
    total_lf_hf = lf + hf

    if total_lf_hf <= 0:
        return None

    lf_nu = (lf / total_lf_hf) * 100.0
    hf_nu = (hf / total_lf_hf) * 100.0
    lf_hf_ratio = lf / hf if hf > 0 else 99.9

    # Peak frequency in LF baroreflex band
    lf_mask = (freqs >= 0.04) & (freqs <= 0.15)
    if np.any(lf_mask):
        lf_freqs = freqs[lf_mask]
        lf_spec = spec[lf_mask]
        peak_idx = np.argmax(lf_spec)
        peak_freq = float(lf_freqs[peak_idx])
        peak_power = float(lf_spec[peak_idx])
        med_lf_power = float(np.median(lf_spec))
    else:
        peak_freq = 0.1
        peak_power = 0.0
        med_lf_power = 1.0

    equivalent_bpm = peak_freq * 60.0

    # Spectral concentration of the dominant LF peak, log-scaled prominence:
    # peak/median ratio 1 -> 0, 10 -> 0.5, 100 -> 1.0. A clean sine-like
    # rhythm yields one sharp peak (high ratio); scattered/noisy LF stays low.
    if med_lf_power > 0 and peak_power > 0:
        prominence = max(1.0, peak_power / med_lf_power)
        peak_concentration = min(1.0, max(0.0, math.log10(prominence) / 2.0))
    else:
        peak_concentration = 0.0

    # Resonance Score = LF dominance x peak concentration (both 0..1)
    score = 100.0 * (lf_nu / 100.0) * peak_concentration

    # Physiological State Determination
    if span < 25.0:
        state = "BUILDING"
    elif score >= 75.0 and 0.06 <= peak_freq <= 0.12:
        state = "RESONANT"
    elif hf_nu >= 50.0 and (lf_nu < 50.0):
        state = "PARASYMPATHETIC"
    elif score < 40.0 and hf_nu < 30.0:
        state = "STRESS/DISRUPTED"
    else:
        state = "BALANCED"

    return {
        "score": score,
        "state": state,
        "lf_nu": lf_nu,
        "hf_nu": hf_nu,
        "lf_hf_ratio": lf_hf_ratio,
        "peak_freq": peak_freq,
        "equivalent_bpm": equivalent_bpm,
        "lf": lf,
        "hf": hf,
        "vlf": vlf,
    }


# --------------------------------------------------------------------------
# Heart Stream Processing
# --------------------------------------------------------------------------

class HeartStream:
    def __init__(self, window_beats: int = 240):
        self.window_beats = window_beats
        self.ibis: list[tuple[float, float]] = []  # (beat_time_sec, ibi_ms)
        self.raw_ibis: list[float] = []
        self.last_hr = 0.0
        self.last_ibi = 0
        self.artifacts = 0
        self.records = 0
        self.session_started = False
        self.started_wall = time.time()
        self.beat_time_accum = 0.0
        self._buf = b""

    def feed(self, payload: bytes) -> None:
        self._buf += payload
        while b"\r" in self._buf:
            line, self._buf = self._buf.split(b"\r", 1)
            line = line.strip()
            if not line:
                continue
            self._emit(line)

    def _emit(self, line: bytes) -> None:
        self.records += 1
        if SESSIONSTART_RE.search(line):
            self.session_started = True

        # Process qualified <2 I=ms R=flag H=hr> records ONLY to avoid duplication
        for m in IBI_RE.finditer(line):
            ibi_ms = float(int(m.group(1)))
            flag = m.group(2)
            hr = int(m.group(3))

            self.last_ibi = int(ibi_ms)
            if hr > 0:
                self.last_hr = float(hr)

            # Advance physiological beat time axis
            step_sec = ibi_ms / 1000.0
            self.beat_time_accum += step_sec

            if flag == b"T":
                self.artifacts += 1
            else:
                # Store valid NN interval with exact cumulative beat time
                self.ibis.append((self.beat_time_accum, ibi_ms))
                if len(self.ibis) > self.window_beats:
                    self.ibis = self.ibis[-self.window_beats:]

            self.raw_ibis.append(ibi_ms)
            if len(self.raw_ibis) > self.window_beats:
                self.raw_ibis = self.raw_ibis[-self.window_beats:]

    def metrics(self) -> dict:
        m = hrv_metrics(self.ibis)
        res = resonance_analysis(self.ibis)
        return {
            "hr": self.last_hr,
            "ibi": self.last_ibi,
            "hrv": m,
            "res": res,
            "beats": len(self.ibis),
            "artifacts": self.artifacts,
            "records": self.records,
            "session": self.session_started,
            "elapsed": time.time() - self.started_wall,
        }


# --------------------------------------------------------------------------
# Breath Pacer Helper (6.0 bpm: 4s inhale, 6s exhale)
# --------------------------------------------------------------------------

def get_pacer_state(elapsed_sec: float, bpm: float = 6.0) -> tuple[str, float, str]:
    cycle_sec = 60.0 / bpm
    inhale_sec = cycle_sec * 0.4
    pos = elapsed_sec % cycle_sec

    if pos < inhale_sec:
        progress = pos / inhale_sec
        phase = "INHALE"
    else:
        progress = (pos - inhale_sec) / (cycle_sec - inhale_sec)
        phase = "EXHALE"

    bar_len = 16
    filled = int(progress * bar_len)
    if phase == "INHALE":
        visual = f"[{'=' * filled}{'>' if filled < bar_len else ''}{' ' * (bar_len - filled - 1)}]"
    else:
        visual = f"[{' ' * (bar_len - filled - 1)}{'<' if filled < bar_len else ''}{'=' * filled}]"

    return phase, progress, visual


# --------------------------------------------------------------------------
# Curses TUI Render
# --------------------------------------------------------------------------

def draw(stdscr, st: HeartStream) -> None:
    stdscr.erase()
    h, w = stdscr.getmaxyx()
    m = st.metrics()
    hrv, res = m["hrv"], m["res"]

    # 1. Header Line
    hr_s = f"{m['hr']:.0f}" if m["hr"] else "--"
    header = f" emWave2 Resonance Trainer | HR {hr_s:>3} bpm | IBI {m['ibi']:>4} ms | Beats {m['beats']}"
    try:
        stdscr.addstr(0, 0, header[:w - 1], curses.A_BOLD)
    except curses.error:
        pass

    # 2. Pacer & State Line
    phase, _, pacer_bar = get_pacer_state(m["elapsed"], bpm=6.0)
    state_str = res["state"] if res else "BUILDING"
    score_str = f"{res['score']:5.1f}%" if res else " -- %"
    pacer_line = f" Pacer (6.0 bpm): {pacer_bar} {phase:<6} | State: {state_str:<14} | Score: {score_str}"
    try:
        stdscr.addstr(1, 0, pacer_line[:w - 1])
    except curses.error:
        pass

    # 3. Spectral & Frequency Details
    if res:
        spec_line = (f" LF_nu: {res['lf_nu']:5.1f}%  HF_nu: {res['hf_nu']:5.1f}%  "
                     f"LF/HF: {res['lf_hf_ratio']:4.2f}  Peak: {res['peak_freq']:.3f} Hz ({res['equivalent_bpm']:.1f} bpm)")
        try:
            stdscr.addstr(2, 0, spec_line[:w - 1])
        except curses.error:
            pass

    # 4. Graph Area
    gh = h - 8
    if gh > 2 and w > 30:
        ibis = st.raw_ibis[- (w - 2):]
        if ibis:
            lo = min(ibis)
            hi = max(ibis)
            if hi - lo < 100:
                hi = lo + 100
            lo, hi = max(0, lo - 50), hi + 50
            try:
                stdscr.addstr(3, 0, f" IBI Trend (ms) [{lo:.0f} .. {hi:.0f}]:"[:w - 1])
            except curses.error:
                pass
            for i, v in enumerate(ibis):
                if i >= w - 2:
                    break
                row_idx = int((v - lo) / (hi - lo) * (gh - 1))
                col = 1 + i
                for r in range(gh):
                    try:
                        if r == gh - 1 - row_idx:
                            stdscr.addch(4 + r, col, "*")
                    except curses.error:
                        pass

    # 5. Summary Footer
    row = h - 3
    if hrv:
        line_hrv = (f" SDNN: {hrv['sdnn']:5.1f} ms | RMSSD: {hrv['rmssd']:5.1f} ms | "
                    f"pNN50: {hrv['pnn50']:4.1f}% | RSA Ampl: {hrv['hr_max_min']:5.1f} ms")
        try:
            stdscr.addstr(row, 0, line_hrv[:w - 1])
            row += 1
        except curses.error:
            pass

    status_line = f" Elapsed: {int(m['elapsed'])}s | Artifacts: {m['artifacts']} | Records: {m['records']} | Press 'q' to quit"
    try:
        stdscr.addstr(row, 0, status_line[:w - 1])
    except curses.error:
        pass


def run_tui(seconds: float, window: int) -> int:
    d = EmWave2()
    st = HeartStream(window)
    print("Starting session and breath pacer...")
    d.start_session()

    def main_loop(stdscr):
        try:
            while True:
                rep = d.read_report(150)
                if rep is not None and rep[0] == 0x75:
                    st.feed(rep[4:4 + rep[3]])

                now = time.time()
                if now - last_draw > 0.25:  # 4 Hz UI refresh for smooth pacer
                    draw(stdscr, st)
                    stdscr.refresh()
                    last_draw = now

                ch = stdscr.getch()
                if ch in (ord("q"), ord("Q")):
                    break
                if end_time and now > end_time:
                    break
        except KeyboardInterrupt:
            pass

    try:
        curses.wrapper(main_loop)
    finally:
        d.stop_session()
        d.close()
    return 0


# --------------------------------------------------------------------------
# Debug / Non-TTY Dump Mode
# --------------------------------------------------------------------------

def run_dump(seconds: float, window: int) -> int:
    d = EmWave2()
    st = HeartStream(window)
    d.start_session()
    try:
        end = time.time() + seconds
        last = 0.0
        while time.time() < end:
            rep = d.read_report(200)
            if rep is not None and rep[0] == 0x75:
                st.feed(rep[4:4 + rep[3]])
            now = time.time()
            if now - last >= 1.0:
                m = st.metrics()
                hr = f"{m['hr']:.0f}" if m["hr"] else "--"
                hrv = m["hrv"]
                res = m["res"]
                hrv_s = f"SDNN={hrv['sdnn']:.1f} RMSSD={hrv['rmssd']:.1f}" if hrv else "BUILDING"
                res_s = f"Score={res['score']:.1f}% State={res['state']} LF_nu={res['lf_nu']:.1f}% Peak={res['peak_freq']:.3f}Hz" if res else "BUILDING"
                print(f"t={int(m['elapsed']):4d}s | HR={hr:>3} | IBI={m['ibi']:>4} | Beats={m['beats']:3d} | {hrv_s:<28} | {res_s}", flush=True)
                last = now
    except KeyboardInterrupt:
        pass
    finally:
        d.stop_session()
        d.close()
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description="emWave2 Research-Validated HRV Trainer")
    ap.add_argument("--seconds", type=float, default=0.0, help="Auto-exit after N seconds (0 = until 'q')")
    ap.add_argument("--window", type=int, default=240, help="Rolling IBI window length in beats")
    ap.add_argument("--dump", type=float, default=0.0, help="Run non-curses dump mode for N seconds")
    args = ap.parse_args()

    if args.dump > 0:
        return run_dump(args.dump, args.window)
    return run_tui(args.seconds, args.window)


if __name__ == "__main__":
    sys.exit(main())
