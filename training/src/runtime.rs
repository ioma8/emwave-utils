use std::time::Instant;

use crate::metrics::{self, TrialAlignment};
use crate::Snapshot;

pub(crate) const FINDER_RATES: [f64; 5] = [6.5, 6.0, 5.5, 5.0, 4.5];
pub(crate) const TRIAL_SECONDS: f64 = 120.0;
pub(crate) const REST_SECONDS: f64 = 120.0;
pub(crate) const MIN_TRIAL_DATA_SECONDS: f64 = 108.0;
pub(crate) const PARTIAL_MIN_TRIAL_SECONDS: f64 = 30.0;

#[derive(Clone)]
pub(crate) struct TrialResult {
    pub(crate) rate: f64,
    pub(crate) alignment: Option<TrialAlignment>,
}

pub(crate) struct FinderRuntime {
    pub(crate) active: bool,
    pub(crate) started: Option<Instant>,
    pub(crate) rest_started: Option<Instant>,
    pub(crate) resting: bool,
    pub(crate) data_started_s: Option<f64>,
    pub(crate) rate_index: usize,
    pub(crate) run_all: bool,
    pub(crate) interrupted: bool,
    pub(crate) results: Vec<TrialResult>,
}

impl Default for FinderRuntime {
    fn default() -> Self {
        Self {
            active: false,
            started: None,
            rest_started: None,
            resting: false,
            data_started_s: None,
            rate_index: 0,
            run_all: false,
            interrupted: false,
            results: Vec::new(),
        }
    }
}

impl FinderRuntime {
    pub(crate) fn start_trial(
        &mut self,
        rate_index: usize,
        run_all: bool,
        snap: &Snapshot,
    ) {
        self.active = true;
        self.started = Some(Instant::now());
        self.rest_started = None;
        self.resting = false;
        self.data_started_s = snap.analysis_received.last().map(|&(time, _)| time);
        self.rate_index = rate_index.min(FINDER_RATES.len() - 1);
        self.run_all = run_all;
        self.interrupted = false;
    }

    pub(crate) fn finish_trial(&mut self, snap: &Snapshot, minimum_span_s: f64) {
        let rate = FINDER_RATES[self.rate_index];
        let alignment = self.data_started_s.and_then(|start| {
            metrics::trial_alignment(
                &snap.analysis_received,
                start,
                rate,
                minimum_span_s,
            )
        });
        if let Some(alignment) = alignment {
            let result = TrialResult {
                rate,
                alignment: Some(alignment),
            };
            if let Some(existing) = self
                .results
                .iter_mut()
                .find(|existing| (existing.rate - rate).abs() < 0.01)
            {
                *existing = result;
            } else {
                self.results.push(result);
            }
        }

        let next = self.rate_index + 1;
        if self.run_all && next < FINDER_RATES.len() {
            self.rate_index = next;
            self.started = None;
            self.rest_started = Some(Instant::now());
            self.resting = true;
        } else {
            self.active = false;
            self.started = None;
            self.rest_started = None;
            self.resting = false;
            self.data_started_s = None;
            self.run_all = false;
        }
    }

    pub(crate) fn update(&mut self, snap: &Snapshot) {
        if !self.active {
            return;
        }
        if !snap.connected {
            self.run_all = false;
            if !self.resting {
                self.finish_trial(snap, PARTIAL_MIN_TRIAL_SECONDS);
            } else {
                self.active = false;
                self.rest_started = None;
                self.resting = false;
            }
            self.interrupted = true;
            return;
        }
        if self.resting {
            if self
                .rest_started
                .is_some_and(|started| started.elapsed().as_secs_f64() >= REST_SECONDS)
            {
                let next = self.rate_index;
                self.start_trial(next, true, snap);
            }
            return;
        }
        if self
            .started
            .is_some_and(|started| started.elapsed().as_secs_f64() >= TRIAL_SECONDS)
        {
            self.finish_trial(snap, MIN_TRIAL_DATA_SECONDS);
        }
    }
}
