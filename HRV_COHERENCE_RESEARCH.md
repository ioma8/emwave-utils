# HRV coherence & biofeedback: validated formulas for emwave_tui

## Bottom line

`emwave_tui.py` reports **standardized HRV metrics** from the 1996 Task Force report and
subsequent peer-reviewed standards, plus a **baroreflex-resonance score** grounded in the
resonance-frequency biofeedback literature. It does **not** use HeartMath's proprietary
coherence formula (excluded per project decision; see §5).

What it can honestly show in 30–120 s: signal quality, heart rate, RMSSD/SDNN (descriptive),
and whether the heart-rhythm power is concentrating around the paced-breathing frequency
(resonance alignment). What it cannot show: "emotion", stress, autonomic balance, or any
diagnostic state — none of those are inferable from a single short PPG-IBI stream.

## 1. Standards basis (primary sources)

1. **Task Force of the ESC & NASPE (1996)** — *Heart rate variability: standards of
   measurement, physiological interpretation and clinical use.* Circulation 93(5):1043–1065.
   [PMID 8598068](https://pubmed.ncbi.nlm.nih.gov/8598068/)
2. **Shaffer & Ginsberg (2017)** — *An Overview of Heart Rate Variability Metrics and Norms.*
   Front. Public Health 5:258. [PMC5624990](https://pmc.ncbi.nlm.nih.gov/articles/PMC5624990/)
3. **Shaffer & Meehan (2020)** — *A Practical Guide to Resonance Frequency Assessment for
   Heart Rate Variability Biofeedback.* Front. Neurosci. 14:570400.
   [PMC7578229](https://pmc.ncbi.nlm.nih.gov/articles/PMC7578229/)
4. **Lehrer & Gevirtz (2014)**, **Vaschillo et al. (2002, 2006)** — baroreflex resonance model.

## 2. Time-domain metrics (exact formulas)

$NN_i$ = consecutive normal-to-normal (artifact-free) inter-beat intervals, ms.

- **Mean RR / Mean HR**: $\overline{NN} = \frac1N\sum NN_i$; $\text{HR} = 60000/\overline{NN}$.
- **SDNN** = $\sqrt{\frac{1}{N-1}\sum(NN_i-\overline{NN})^2}$ — total variability; standard is
  5 min, but short-window values are descriptive only.
- **RMSSD** = $\sqrt{\frac{1}{N-1}\sum_{i=1}^{N-1}(NN_{i+1}-NN_i)^2}$ — vagally mediated;
  robust to 30–60 s; preferred short-term parasympathetic index.
- **pNN50** = percentage of $|NN_{i+1}-NN_i| > 50$ ms.
- **RSA amplitude (HR max − HR min)** = mean peak-trough HR per breath cycle.

Sources: Task Force 1996; Shaffer & Ginsberg 2017 (Table 1, §Time-Domain).

## 3. Frequency-domain metrics

### Preprocessing (Task Force 1996 method)
1. Reconstruct beat times as cumulative NN sums, not wall-clock arrival (removes jitter).
2. Interpolate to a uniform **4 Hz** grid.
3. Detrend (remove mean), apply **Hanning** window, FFT → PSD.

### Bands (Task Force 1996)
| Band | Range | Period / breathing |
|---|---|---|
| VLF | 0.0033–0.04 Hz | 25–300 s (needs >5 min to be valid) |
| LF | 0.04–0.15 Hz | 6.7–25 s, ~4–9 bpm — baroreflex/resonance range |
| HF | 0.15–0.40 Hz | 2.5–6.7 s, ~9–24 bpm — RSA/vagal |

### Normalized units & ratio
$\text{LF}_{\text{nu}} = \dfrac{\text{LF}}{\text{LF}+\text{HF}}\cdot100$,
$\text{HF}_{\text{nu}} = \dfrac{\text{HF}}{\text{LF}+\text{HF}}\cdot100$,
$\text{LF/HF} = \text{LF}/\text{HF}$.

Sources: Task Force 1996; Shaffer & Ginsberg 2017 (Table 2, §Frequency-Domain).

## 4. Resonance score (validated concept, open formula)

During resonance-frequency breathing (adults **4.5–6.5 bpm**, peak frequency
0.075–0.11 Hz) HRV power concentrates into a single sharp peak in the LF band
(Shaffer & Meehan 2020; Lehrer et al. 2020). The trainer reports:

- **peak frequency** $f_p$ = argmax of PSD in 0.04–0.15 Hz, plus its bpm equivalent;
- **LF dominance** = $\text{LF}_{\text{nu}}$;
- **peak concentration** $c = \min\left(1,\ \max\left(0,\ \dfrac{\log_{10}(P_{\text{peak}}/P_{\text{med,LF}})}{2}\right)\right)$,
  a log-scaled prominence of the dominant LF peak over the median LF power
  (ratio 1→0, 10→0.5, 100→1.0) — discriminates a clean sine-like rhythm from
  scattered/noisy LF energy;
- **resonance score (0–100)** = $\text{LF}_{\text{nu}} \times c$.

This is a transparent heuristic derived from validated LF-dominance and
spectral-peak-sharpness concepts — not a peer-reviewed clinical score.

### State labels (physiological, not emotional)
| State | Criterion |
|---|---|
| BUILDING | < 25 s span (spectral estimate unstable) |
| RESONANT | score ≥ 75 and $0.06\le f_p\le 0.12$ Hz |
| PARASYMPATHETIC | $\text{HF}_{\text{nu}}\ge 50\%$ |
| STRESS/DISRUPTED | score < 40 and $\text{HF}_{\text{nu}}<30\%$ (or high artifact rate) |
| BALANCED | otherwise |

Sources: Shaffer & Meehan 2020 (resonance band, LF peak, peak sharpness criteria);
Shaffer & Ginsberg 2017 (HF = parasympathetic).

## 5. Why HeartMath's formula is not used

HeartMath's coherence ratio — peak power in a 0.030-Hz window centered on the max peak in
0.04–0.26 Hz, divided by (total − peak) — is described publicly (McCraty 2022,
[PMC9214473](https://pmc.ncbi.nlm.nih.gov/articles/PMC9214473/)) but the exact
preprocessing, windowing, artifact correction, and ratio→Low/Medium/High score mapping are
proprietary and not reproducible. Per project decision, `emwave_tui.py` uses only the
standardized, independently validated metrics above and does not claim emWave-equivalent
scores.

## 6. Session-length guidance

- **RMSSD**: meaningful from ~30 s (Shaffer & Ginsberg 2017; ultra-short-term proposals).
- **LF/HF, LF peak, resonance score**: require ≥ ~2 min for stable spectral estimates;
  64–120 s gives an initial resonance readout, not a clinical value.
- **VLF**: ≥ 5 min minimum — shown for completeness, labeled unreliable at shorter spans.
- **SDNN**: 5 min standard; short-window value is descriptive.

## 7. Artifact correction

Only `R=F` (clean) beats enter the NN series. `R=T` beats advance the physiological time
axis (their IBI duration is added) but are excluded from all metrics and counted as
artifacts. This mirrors the standard "remove ectopic/artifactual beats, preserve timing"
guidance (Task Force 1996; Shaffer & Meehan 2020 §Resonance Frequency Trials).
