//! Validated HRV / baroreflex-resonance math.
//!
//! Time-domain (SDNN, RMSSD, pNN50) and frequency-domain (LF/HF, LF_nu, peak)
//! metrics per the 1996 Task Force and Shaffer 2017/2020 standards. See
//! ../HRV_COHERENCE_RESEARCH.md. No FFT dependency: the 4 Hz series is short
//! (~256 samples), so a direct DFT is fast enough and dead simple.

use crate::analysis::NnSeries;
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, Default)]
pub struct HrvMetrics {
    pub sdnn: f64,
    pub rmssd: f64,
    pub pnn50: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Resonance {
    pub score: f64,
    pub lf_nu: f64,
    pub hf_nu: f64,
    pub bpm: f64,
    pub state: &'static str,
}

fn detrend_hann(values: &mut [f64]) {
    if values.len() < 2 {
        return;
    }
    let n = values.len() as f64;
    let mean_x = (n - 1.0) / 2.0;
    let mean_y = values.iter().sum::<f64>() / n;
    let denominator = values
        .iter()
        .enumerate()
        .map(|(i, _)| (i as f64 - mean_x).powi(2))
        .sum::<f64>();
    let slope = values
        .iter()
        .enumerate()
        .map(|(i, &value)| (i as f64 - mean_x) * (value - mean_y))
        .sum::<f64>()
        / denominator;
    let intercept = mean_y - slope * mean_x;
    let last = (values.len() - 1) as f64;
    for (i, value) in values.iter_mut().enumerate() {
        let trend = intercept + slope * i as f64;
        let window = 0.5 * (1.0 - (2.0 * PI * i as f64 / last).cos());
        *value = (*value - trend) * window;
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TrialAlignment {
    pub score: f64,
    pub peak_bpm: f64,
    pub mismatch_bpm: f64,
    pub target_peak_ratio: f64,
    pub lf_nu: f64,
    pub span_s: f64,
    pub reliable: bool,
}

/// Time-domain metrics over artifact-free NN intervals (beat_time_s, ibi_ms).
pub fn hrv_metrics(series: &NnSeries) -> Option<HrvMetrics> {
    let ibis = series.as_slice();
    let n = ibis.len();
    if n < 4 {
        return None;
    }
    let nn: Vec<f64> = ibis.iter().map(|&(_, m)| m).collect();
    let mean_rr = nn.iter().sum::<f64>() / n as f64;

    let var = nn.iter().map(|v| (v - mean_rr).powi(2)).sum::<f64>();
    let sdnn = (var / (n - 1) as f64).sqrt();

    let (mut ssd, mut nn50) = (0.0, 0.0);
    for i in 1..n {
        let d = nn[i] - nn[i - 1];
        ssd += d * d;
        if d.abs() > 50.0 {
            nn50 += 1.0;
        }
    }
    let rmssd = (ssd / (n - 1) as f64).sqrt();
    let pnn50 = nn50 / (n - 1) as f64 * 100.0;


    Some(HrvMetrics {
        sdnn,
        rmssd,
        pnn50,
    })
}

/// Frequency-domain analysis + resonance score over clean NN intervals.
pub fn resonance(series: &NnSeries) -> Option<Resonance> {
    resonance_raw(series.as_slice())
}

fn resonance_raw(ibis: &[(f64, f64)]) -> Option<Resonance> {
    let n = ibis.len();
    if n < 16 {
        return None;
    }
    let t: Vec<f64> = ibis.iter().map(|&(t, _)| t).collect();
    let y: Vec<f64> = ibis.iter().map(|&(_, m)| m).collect();
    let span = t[n - 1] - t[0];
    if span < 12.0 {
        return None;
    }

    let fs = 4.0;
    let npts = (span * fs) as usize;
    if npts < 16 {
        return None;
    }

    // Linear-interpolate NN to a uniform 4 Hz grid.
    let mut yi = Vec::with_capacity(npts);
    let mut j = 0usize;
    for k in 0..npts {
        let x = t[0] + k as f64 / fs;
        while j + 1 < n && t[j + 1] < x {
            j += 1;
        }
        let (t0, t1) = (t[j], t[j + 1]);
        let v = if t1 > t0 {
            y[j] + (y[j + 1] - y[j]) * (x - t0) / (t1 - t0)
        } else {
            y[j]
        };
        yi.push(v);
    }

    // Remove linear drift before applying the Hann taper.
    detrend_hann(&mut yi);

    // Direct DFT -> power spectrum.
    let nf = npts / 2;
    let mut power = vec![0.0f64; nf];
    for k in 0..nf {
        let wk = 2.0 * PI * k as f64 / npts as f64;
        let (mut re, mut im) = (0.0, 0.0);
        for (idx, &v) in yi.iter().enumerate() {
            re += v * (wk * idx as f64).cos();
            im -= v * (wk * idx as f64).sin();
        }
        power[k] = re * re + im * im;
    }
    let freq = |k: usize| k as f64 * fs / npts as f64;

    let band = |f0: f64, f1: f64| -> f64 {
        (0..nf)
            .filter(|&k| {
                let f = freq(k);
                f >= f0 && f < f1
            })
            .map(|k| power[k])
            .sum()
    };

    let vlf = band(0.0033, 0.04);
    let lf = band(0.04, 0.15);
    let hf = band(0.15, 0.40);
    let total_lf_hf = lf + hf;
    if total_lf_hf <= 0.0 {
        return None;
    }
    let _ = vlf; // VLF needs >5 min to be valid; computed but not surfaced.

    let lf_nu = lf / total_lf_hf * 100.0;
    let hf_nu = hf / total_lf_hf * 100.0;
    // LF/HF remains available through the band powers; no UI consumes the ratio.

    // Dominant peak within the LF band + its prominence over the median LF bin.
    let mut peak_freq = 0.1;
    let mut peak_power = 0.0;
    let mut lf_powers: Vec<f64> = Vec::new();
    for k in 0..nf {
        let f = freq(k);
        if (0.04..=0.15).contains(&f) {
            lf_powers.push(power[k]);
            if power[k] > peak_power {
                peak_power = power[k];
                peak_freq = f;
            }
        }
    }
    lf_powers.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med_lf = lf_powers[lf_powers.len() / 2];

    // Log-scaled peak concentration: ratio 1->0, 10->0.5, 100->1.0.
    let peak_concentration = if med_lf > 0.0 && peak_power > 0.0 {
        let prominence = (peak_power / med_lf).max(1.0);
        (prominence.log10() / 2.0).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let score = lf_nu * peak_concentration; // 0..100
    let bpm = peak_freq * 60.0;

    let state = if span < 25.0 {
        "BUILDING"
    } else if score >= 75.0 && (0.06..=0.12).contains(&peak_freq) {
        "RESONANT"
    } else if hf_nu >= 50.0 {
        "PARASYMPATHETIC"
    } else if score < 40.0 && hf_nu < 30.0 {
        "STRESS"
    } else {
        "BALANCED"
    };

    Some(Resonance {
        score,
        lf_nu,
        hf_nu,
        bpm,
        state,
    })
}

/// Scores a trial against its exact paced-breathing frequency.
///
/// `start_s` is the physiological timestamp captured when the trial began.
/// A result is reliable only when its strongest LF peak is within 0.5 bpm of
/// the paced rate and the target-frequency score is at least 35/100.
pub fn trial_alignment(
    series: &NnSeries,
    start_s: f64,
    target_bpm: f64,
    minimum_span_s: f64,
) -> Option<TrialAlignment> {
    let ibis = series.as_slice();
    let first = ibis.partition_point(|&(t, _)| t < start_s);
    let data = &ibis[first..];
    let n = data.len();
    if n < 16 {
        return None;
    }
    let span = data[n - 1].0 - data[0].0;
    if span < minimum_span_s {
        return None;
    }

    let base = resonance_raw(data)?;
    let fs = 4.0;
    let npts = (span * fs) as usize;
    let mut yi = Vec::with_capacity(npts);
    let mut j = 0usize;
    for k in 0..npts {
        let x = data[0].0 + k as f64 / fs;
        while j + 1 < n && data[j + 1].0 < x {
            j += 1;
        }
        if j + 1 >= n {
            break;
        }
        let (t0, y0) = data[j];
        let (t1, y1) = data[j + 1];
        yi.push(if t1 > t0 {
            y0 + (y1 - y0) * (x - t0) / (t1 - t0)
        } else {
            y0
        });
    }
    if yi.len() < 16 {
        return None;
    }
    detrend_hann(&mut yi);
    let power_at = |frequency: f64| {
        let omega = 2.0 * PI * frequency / fs;
        let (mut re, mut im) = (0.0, 0.0);
        for (index, &value) in yi.iter().enumerate() {
            re += value * (omega * index as f64).cos();
            im -= value * (omega * index as f64).sin();
        }
        re * re + im * im
    };

    let step = (1.0 / span) / 4.0;
    let mut lf_powers = Vec::new();
    let (mut peak_frequency, mut peak_power) = (0.04, 0.0);
    let mut frequency = 0.04;
    while frequency <= 0.15 {
        let power = power_at(frequency);
        lf_powers.push(power);
        if power > peak_power {
            peak_frequency = frequency;
            peak_power = power;
        }
        frequency += step;
    }
    lf_powers.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median_lf = lf_powers[lf_powers.len() / 2];
    let target_power = power_at(target_bpm / 60.0);
    let prominence = if median_lf > 0.0 {
        (target_power / median_lf).max(1.0)
    } else {
        1.0
    };
    let concentration = (prominence.log10() / 2.0).clamp(0.0, 1.0);
    let peak_bpm = peak_frequency * 60.0;
    let mismatch_bpm = (peak_bpm - target_bpm).abs();
    let target_peak_ratio = if peak_power > 0.0 {
        target_power / peak_power
    } else {
        0.0
    };
    // Score the target response independently; mismatch only controls status.
    let score = base.lf_nu * concentration;
    let reliable = mismatch_bpm <= 0.5 && score >= 35.0;
    Some(TrialAlignment {
        score,
        peak_bpm,
        mismatch_bpm,
        target_peak_ratio,
        lf_nu: base.lf_nu,
        span_s: span,
        reliable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paced_ibis(rate_bpm: f64) -> Vec<(f64, f64)> {
        let mut samples = Vec::new();
        let mut time = 0.0;
        while time < 125.0 {
            let ibi = 1000.0 + 120.0 * (2.0 * PI * rate_bpm / 60.0 * time).sin();
            time += ibi / 1000.0;
            samples.push((time, ibi));
        }
        samples
    }

    #[test]
    fn target_frequency_matches_and_wrong_frequency_is_rejected() {
        let ibis = paced_ibis(5.5);
        let matched = trial_alignment(&NnSeries::from_vec(ibis.clone()), 0.0, 5.5, 108.0).unwrap();
        let wrong = trial_alignment(&NnSeries::from_vec(ibis), 0.0, 7.0, 108.0).unwrap();

        assert!(matched.reliable);
        assert!(matched.mismatch_bpm < 0.2);
        assert!(!wrong.reliable);
        assert!(wrong.score < matched.score);
    }

    #[test]
    fn time_domain_metrics_use_sample_variance_and_successive_differences() {
        let metrics = hrv_metrics(&NnSeries::from_vec(vec![
            (0.0, 1000.0),
            (1.0, 1010.0),
            (2.0, 990.0),
            (3.0, 1000.0),
        ]))
        .unwrap();
        assert!((metrics.sdnn - (200.0_f64 / 3.0).sqrt()).abs() < 1e-9);
        assert!((metrics.rmssd - 200.0_f64.sqrt()).abs() < 1e-9);
        assert_eq!(metrics.pnn50, 0.0);
    }
    #[test]
    fn off_target_peak_keeps_nonzero_target_score() {
        let mut samples = Vec::new();
        let mut time = 0.0;
        while time < 125.0 {
            let dominant = 180.0 * (2.0 * PI * 2.8 / 60.0 * time).sin();
            let target = 80.0 * (2.0 * PI * 5.5 / 60.0 * time).sin();
            let ibi = 1000.0 + dominant + target;
            time += ibi / 1000.0;
            samples.push((time, ibi));
        }
        let result = trial_alignment(&NnSeries::from_vec(samples), 0.0, 5.5, 108.0).unwrap();
        assert!(!result.reliable);
        assert!(result.score > 0.0);
        assert!(result.mismatch_bpm > 0.5);
    }
    #[test]
    fn short_partial_window_still_returns_response_score() {
        let ibis: Vec<_> = paced_ibis(5.5)
            .into_iter()
            .take_while(|&(time, _)| time <= 35.0)
            .collect();
        let result = trial_alignment(&NnSeries::from_vec(ibis), 0.0, 5.5, 30.0).unwrap();
        assert!(result.span_s < 108.0);
        assert!(result.score > 0.0);
        assert!(!result.reliable);
    }
}
