# HRV coherence & biofeedback: validated formulas for Resonance

## Bottom line

The Rust app reports standardized HRV metrics and a transparent
cardiac PPG target-frequency heuristic. It does not use HeartMath's proprietary
coherence formula (excluded per project decision; see §5).

What it can honestly show in 30–120 s: signal quality, heart rate, RMSSD/SDNN
(descriptive), and whether heart-rhythm power concentrates around a paced
breathing frequency. What it cannot show without synchronized respiration:
phase synchrony, validated personal resonance frequency, autonomic balance, or
baroreflex gain.
It does not infer diagnostic states or establish personal resonance frequency; that
requires separate paced trials with synchronized respiration measurement.

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
proprietary and not reproducible. Per project decision, the Rust app uses only
the standardized metrics and transparent cardiac target-frequency heuristic; it does not
claim emWave-equivalent scores.

## 6. What the app's 240-beat window means

`HeartStream(window_beats=240)` is a **beat-count window, not a time window**. On each
qualified IBI record, the stream advances its cumulative physiological time. Records
whose flag is not `T` are appended to `ibis`, and once more than 240 such clean NN
intervals are present only the most recent 240 remain. `T` records advance the time axis
and are counted as artifacts, but are excluded from `ibis` and therefore from all metrics.
`hrv_metrics()` and `resonance()` recompute on that latest clean-interval set. The Rust
finder uses receipt-time boundaries for its explicit multi-rate trials.

The represented duration therefore varies with heart rate:

| Mean HR | Approximate duration of 240 beats |
|---:|---:|
| 50 bpm | 4.8 min |
| 60 bpm | 4.0 min |
| 75 bpm | 3.2 min |
| 100 bpm | 2.4 min |

This is an estimate (`240 × 60 / HR`); artifact gaps can make elapsed wall/physiological
time longer, while the number of usable NN intervals remains capped at 240.

## 7. Evidence on duration and window choice (non-HeartMath)

The following separates what the sources found from what we recommend for this app.

| Metric / analysis | Evidence | Consequence for this app |
|---|---|---|
| **RMSSD** | The Task Force standard is a 5-min short-term recording, but later validation studies support shorter, purpose-specific estimates. In a large adult sample (*N* = 3,387), Munoz et al. found no benefit from recordings longer than 120 s for RMSSD and found a single 10-s ECG could be valid, although averaging several 10-s segments was preferable. Melo et al. found RMSSD was the most reliable index across 1-, 2-, and 3-min epochs versus 6 min, but controlled breathing changed mean values. [Laborde et al.](https://doi.org/10.3389/fpsyg.2017.00213) summarize the evidence; primary studies: [Munoz et al. 2015](https://doi.org/10.1371/journal.pone.0138921), [Melo et al. 2018](https://doi.org/10.1111/anec.12565). | RMSSD can be shown as an **ultra-short descriptive/training signal** after at least ~60 s of clean data, with a stable fixed-time window preferred for comparisons. Do not call a 10–120 s value interchangeable with a 5-min standard. |
| **SDNN** | SDNN reflects all cyclic variability in the analyzed period and is therefore more duration-dependent than RMSSD. The Task Force/consensus recommendation remains 5 min for short-term comparability. In Melo et al., RMSSD was consistently more reliable than SDNN in spontaneous breathing; SDNN agreement improved with paced breathing but short epochs still changed means. [Task Force 1996](https://doi.org/10.1161/01.CIR.93.5.1043), [Melo et al. 2018](https://doi.org/10.1111/anec.12565). | Treat SDNN from the rolling 240 beats as **descriptive only**. For a standards-comparable SDNN display, use a clean, stationary 5-min fixed-time segment. |
| **LF, HF, and LF/HF** | The consensus guidance recommends 5 min for short-term recording and says 1 min is an absolute minimum for reliable HF when a shorter design is unavoidable. Its rationale is at least 10 cycles of the lowest frequency being analyzed. Melo et al. found LF, HF, and LF:HF were poorly correlated with the 6-min reference across 1–3 min epochs, in both spontaneous and paced breathing. [Laborde et al. 2017](https://doi.org/10.3389/fpsyg.2017.00213), [Melo et al. 2018](https://doi.org/10.1111/anec.12565). | Do not present LF/HF or LF/HF-derived labels as stable before 5 min. At 0.04 Hz, ten cycles already require about 250 s; 5 min gives margin and remains the interoperable standard. |
| **VLF** | The Task Force band starts at 0.0033 Hz (period about 300 s); short-term recordings cannot estimate it adequately. [Task Force 1996](https://doi.org/10.1161/01.CIR.93.5.1043). | Keep VLF out of short-session interpretation; if computed, label it unreliable below 5 min. |
| **Spectral “coherence”** | Scientific cross-spectral/wavelet coherence quantifies time-frequency coupling between **two signals**, such as heart rate and respiration. Keissar, Davrath & Akselrod present wavelet-transform coherence specifically as a heart–respiration analysis and discuss thresholds for its estimates; it is not a single-series peak-prominence statistic. [Keissar et al. 2009](https://doi.org/10.1098/rsta.2008.0273). | The current `peak_concentration` is a transparent **single-series LF peak-concentration heuristic**, not standard cross-spectral coherence. It should not be called a coherence measurement unless a synchronized respiratory signal is added and a two-signal coherence method is implemented. |

### Fixed-time versus beat-count windows

For the same person at changing HR, a 240-beat window changes duration and therefore
changes frequency resolution, number of respiratory/baroreflex cycles, and the amount of
slow variability included. A fixed-time window gives comparable frequency resolution and
band coverage across sessions and participants. A beat-count window is defensible for a
responsive biofeedback display, but it is not the usual basis for comparing LF/HF, SDNN,
or spectral peak values between sessions.

### Rolling-window recommendation

**Evidence:** Laborde et al. describe 5-min moving windows (for example, updated every
minute) as a possible way to analyze long recordings, but advise strict 5-min epochs
because moving averages create interpretive difficulties. This is guidance, not a
validation of this app's exact update cadence.

**Recommendation:** Keep the rolling display for responsiveness, but:

1. Prefer a **fixed 5-min time window** for the primary LF/HF, SDNN, and resonance
   estimate; use a 1-min update cadence (or slower) and label values as rolling.
2. Permit a shorter 60–120 s RMSSD-only readout for immediate feedback, explicitly
   marked ultra-short/descriptive.
3. Do not infer a state or compare sessions until the relevant fixed-time window is
   complete, stationary, and artifact-qualified.
4. For research/export comparisons, report the window duration, update/overlap policy,
   clean-beat count, artifact handling, breathing rate, and whether the estimate is
   fixed or rolling.
## 8. Determining an individual's resonance frequency (non-HeartMath protocol)

### What is actually established

The strongest practical, non-commercial protocol is the **Lehrer/Vaschillo resonance-frequency
assessment**, operationalized in the peer-reviewed protocol paper by Lehrer et al. (2013) and
described with its limitations by Shaffer & Meehan (2020). It is a physiological assessment
protocol, not a diagnostic test and not a universally validated gold standard. The underlying
primary evidence is narrower: Vaschillo et al. (2002) trained five healthy men at seven
frequencies from 0.01–0.14 Hz and found high-amplitude, frequency-dependent heart-rate and
blood-pressure oscillations with individual differences in the peak; this supports an
individual resonance phenomenon, but does not validate the later five-trial scan. Lehrer et
al. (2003) studied 54 healthy adults over 10 biofeedback sessions and found acute increases
in LF HRV and vagal baroreflex gain during slow breathing; that study supports the mechanism
and training response, not the reliability or diagnostic accuracy of a two-minute scan.

### Exact Lehrer-style assessment

1. **Prepare the participant.** Seat the participant comfortably and explain that the goal is
   to find the breathing rate that produces the largest, smoothest heart-rate oscillation.
   Let the participant practice relaxed breathing around 5.5–6.0 breaths/min first. Breaths
   should be effortless and not excessively deep; use the same inhale/exhale timing at every
   rate (the protocol commonly uses a somewhat longer exhalation). Do not proceed while the
   participant is struggling, dizzy, or overbreathing.
2. **Measure both signals.** Record ECG R–R intervals (preferred) or a validated PPG, **and a
   synchronized respiratory waveform**, normally with a thoracic/abdominal respirometer.
   Display both in real time. The respiratory channel is required to verify that the person
   actually achieved the target rate, to detect breath-holding/overbreathing, and to calculate
   heart-rate/respiration phase. PPG can be acceptable with good perfusion, but pulse transit
   delay and vasoconstriction make ECG preferable during slow breathing.
3. **Run five separate trials.** Record approximately **2 min at each target**, in the
   standardized descending order **6.5, 6.0, 5.5, 5.0, and 4.5 breaths/min** (0.5-breath/min
   steps), with approximately **2 min of rest between trials**. Save each trial as its own
   epoch, rather than pooling rates. Confirm the achieved rate from the respiration signal;
   repeat a trial when the average rate is about 0.25 breaths/min or more from target. Inspect
   ECG/PPG and respiration for artifacts and repeat a contaminated epoch after another rest.
4. **Extend the scan at an edge.** If the apparent peak is at 4.5 or 6.5 breaths/min, add
   trials 0.5 breaths/min beyond that edge (and continue outward until the response falls).
   The adult range is a heuristic operating range, not proof that every adult's peak lies
   inside it; children use a different range in the source protocol.
5. **Select by convergence, not one magic number.** For each clean epoch, compare the following
   criteria, in the source protocol's stated priority order:
   (a) heart-rate/respiration phase synchrony (near 0° in its convention), (b) mean
   **HR Max − HR Min** across respiratory cycles (RSA amplitude), (c) absolute and normalized
   LF power (0.04–0.15 Hz), (d) the highest-amplitude LF spectral peak, (e) smoothness of the
   heart-rate waveform, and (f) the fewest distinct LF peaks. Pick the rate with the best
   overall convergence. The published weights are expert priorities; **they have not been
   experimentally validated**, and adjacent rates often win different criteria. RMSSD may be
   reported as a supplementary vagal index, but it is not a substitute for confirming actual
   respiration or phase.
6. **Confirm the candidate.** During the first training/confirmation session, record **3–5 min**
   at the candidate and at candidate ±0.5 breaths/min. Recheck comfort, achieved respiration
   rate, artifacts, and the same selection criteria. Resolve a tie using comfort and the
   intervention goal, and document the tie rather than presenting the result as exact.

### Order, randomization, stabilization, and baroreflex gain

The Lehrer-style procedure specifies a fixed descending order; it does **not** specify
randomized or counterbalanced trial order. Therefore its result can be affected by practice,
fatigue, habituation, time drift, and carry-over from the preceding rate. A study seeking an
unbiased experimental comparison should preregister and counterbalance/randomize the five
rates, keep rest and posture constant, and use the same artifact and stopping rules. That is
an improvement for causal comparison, not evidence that randomization is part of the
validated Lehrer protocol. Whichever order is used, stabilization means a comfortable
stationary posture, a short practice period, verified target breathing, and a rest interval;
it does not mean that two minutes has been proven sufficient for every endpoint.

The protocol's primary selection signal is **HR oscillation amplitude**, not a directly measured
baroreflex-gain score. A true baroreflex gain estimate requires beat-to-beat blood pressure
alongside R–R intervals and an explicitly reported method (for example, a validated
sequence or spectral/transfer-function analysis; Parati et al. 2000). Vaschillo et al. (2002)
used transfer functions among respiration, heart rate, and blood pressure to characterize
resonance. An HR-only or PPG-IBI-only app can therefore use RSA amplitude and LF peak
alignment as proxies, but must not label them “baroreflex gain.”

### Reliability and limitations

- The two-minute trial length is conventional, not a demonstrated universal minimum. In 38
  healthy undergraduates, Shaffer et al. (2019) found about 90 s adequate for LF power but
  about 180 s needed for normalized LF power against a 5-min reference; no peer-reviewed
  study had established concurrent validity for the key phase-synchrony and HR Max − HR Min
  criteria when Shaffer & Meehan (2020) reviewed the evidence.
- Test–retest evidence is sparse. Fuller et al. (2011) reported a two-week correlation of
  *r* = 0.73 (*d* = 2.14) in only 21 undergraduates (conference report summarized by Shaffer
  & Meehan); replication in larger and more representative samples was explicitly requested.
  Treat the result as a session estimate, not a permanent trait.
- The apparent peak depends on posture, time of day, training/learning, breathing depth and
  inhale/exhale ratio, CO₂ loss from overbreathing, medications, vascular state, age/body
  size, and signal quality. A visually large LF peak can also reflect failure to follow the
  target or an artifact. Slow breathing near 6 breaths/min often produces large RSA even
  without individual scanning, and evidence that individualized resonance training improves
  clinical outcomes more than generic 6-breaths/min training remains preliminary.
- Pacemakers, clinically important rhythm abnormalities, and conditions in which slow
  breathing could worsen acidosis require clinical screening; this is not a self-administered
  diagnostic maneuver.

### Practical recommendation for the app

The app has a PPG-derived IBI stream and an explicit multi-rate finder, but no synchronized
respiration channel and no direct blood-pressure measurement. Its LF peak/concentration and
target-frequency score are consequently transparent single-series heuristics, **not personal
resonance-frequency determination and not baroreflex-gain measurement**. Report the window,
clean-beat count, artifacts, receipt-time coverage, and paced rate.

The current finder implements the cardiac-only subset: five paced rates, two-minute trials,
two-minute rests, artifact filtering, and target-frequency scoring. It must remain labeled
as a candidate cardiac response. A validated resonance assessment would additionally need
a respiratory belt, verified actual rate, phase synchrony, artifact repeats, multi-criterion
selection, and 3–5-minute confirmation at ±0.5 bpm.

**Primary/methodological sources:** [Vaschillo et al. 2002](https://doi.org/10.1023/A:1014587304314);
[Lehrer et al. 2003](https://doi.org/10.1097/01.PSY.0000089200.81962.19);
[Lehrer et al. 2013](https://doi.org/10.5298/1081-5937-41.3.08);
[Shaffer & Meehan 2020](https://doi.org/10.3389/fnins.2020.570400);
[Parati et al. 2000](https://pubmed.ncbi.nlm.nih.gov/10678538/).

## 9. Artifact correction

Only records whose flag is not `T` enter the NN series. `T` beats advance the
physiological time axis (their IBI duration is added) but are excluded from all metrics
and counted as artifacts. This mirrors the standard "remove ectopic/artifactual beats,
preserve timing" guidance (Task Force 1996; Shaffer & Meehan 2020 §Resonance Frequency
Trials).
