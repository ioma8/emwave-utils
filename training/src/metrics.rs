//! Validated HRV / baroreflex-resonance math.
//!
//! Time-domain (SDNN, RMSSD, pNN50) and frequency-domain (LF/HF, LF_nu, peak)
//! metrics per the 1996 Task Force and Shaffer 2017/2020 standards. See
//! ../HRV_COHERENCE_RESEARCH.md. No FFT dependency: the 4 Hz series is short
//! (~256 samples), so a direct DFT is fast enough and dead simple.

use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, Default)]
pub struct HrvMetrics {
    pub sdnn: f64,
    pub rmssd: f64,
    pub pnn50: f64,
    pub hr_max_min: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Resonance {
    pub score: f64,
    pub lf_nu: f64,
    pub hf_nu: f64,
    pub lf_hf: f64,
    pub peak_freq: f64,
    pub bpm: f64,
    pub state: &'static str,
}

/// Time-domain metrics over artifact-free NN intervals (beat_time_s, ibi_ms).
pub fn hrv_metrics(ibis: &[(f64, f64)]) -> Option<HrvMetrics> {
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

    let min = nn.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = nn.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

    Some(HrvMetrics {
        sdnn,
        rmssd,
        pnn50,
        hr_max_min: max - min,
    })
}

/// Frequency-domain analysis + resonance score over clean NN intervals.
pub fn resonance(ibis: &[(f64, f64)]) -> Option<Resonance> {
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

    // Detrend (subtract mean) and apply a Hanning window.
    let mean = yi.iter().sum::<f64>() / npts as f64;
    for (k, v) in yi.iter_mut().enumerate() {
        let w = 0.5 * (1.0 - (2.0 * PI * k as f64 / (npts - 1) as f64).cos());
        *v = (*v - mean) * w;
    }

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
    let lf_hf = if hf > 0.0 { lf / hf } else { 99.9 };

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
        lf_hf,
        peak_freq,
        bpm,
        state,
    })
}
