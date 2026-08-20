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
    pub(crate) alignment: TrialAlignment,
}

#[derive(Default)]
enum FinderState {
    #[default]
    Idle,
    Trial {
        started: Instant,
        data_started_s: Option<f64>,
        run_all: bool,
    },
    Rest {
        started: Instant,
        next_rate: usize,
    },
}

#[derive(Default)]
pub(crate) struct FinderRuntime {
    state: FinderState,
    pub(crate) rate_index: usize,
    pub(crate) interrupted: bool,
    pub(crate) results: Vec<TrialResult>,
}

impl FinderRuntime {
    pub(crate) fn active(&self) -> bool {
        !matches!(self.state, FinderState::Idle)
    }

    pub(crate) fn resting(&self) -> bool {
        matches!(self.state, FinderState::Rest { .. })
    }

    pub(crate) fn run_all(&self) -> bool {
        matches!(self.state, FinderState::Trial { run_all: true, .. })
            || matches!(self.state, FinderState::Rest { .. })
    }

    pub(crate) fn trial_elapsed(&self) -> f64 {
        match self.state {
            FinderState::Trial { started, .. } => started.elapsed().as_secs_f64(),
            _ => 0.0,
        }
    }

    pub(crate) fn rest_elapsed(&self) -> f64 {
        match self.state {
            FinderState::Rest { started, .. } => started.elapsed().as_secs_f64(),
            _ => 0.0,
        }
    }

    pub(crate) fn data_started_s(&self) -> Option<f64> {
        match self.state {
            FinderState::Trial { data_started_s, .. } => data_started_s,
            _ => None,
        }
    }

    pub(crate) fn start_trial(&mut self, rate_index: usize, run_all: bool, snap: &Snapshot) {
        self.rate_index = rate_index.min(FINDER_RATES.len() - 1);
        self.interrupted = false;
        self.state = FinderState::Trial {
            started: Instant::now(),
            data_started_s: snap.analysis_received.last().map(|&(time, _)| time),
            run_all,
        };
    }

    pub(crate) fn finish_trial(&mut self, snap: &Snapshot, minimum_span_s: f64) {
        let rate = FINDER_RATES[self.rate_index];
        let data_started_s = self.data_started_s();
        if let Some(start) = data_started_s {
            if let Some(alignment) =
                metrics::trial_alignment(&snap.analysis_received, start, rate, minimum_span_s)
            {
                let result = TrialResult { rate, alignment };
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
        }

        let next = self.rate_index + 1;
        if self.run_all() && next < FINDER_RATES.len() {
            self.rate_index = next;
            self.state = FinderState::Rest {
                started: Instant::now(),
                next_rate: next,
            };
        } else {
            self.state = FinderState::Idle;
        }
    }

    pub(crate) fn update(&mut self, snap: &Snapshot) {
        if !self.active() {
            return;
        }
        if !snap.connected {
            if self.resting() {
                self.state = FinderState::Idle;
            } else {
                self.finish_trial(snap, PARTIAL_MIN_TRIAL_SECONDS);
                self.state = FinderState::Idle;
            }
            self.interrupted = true;
            return;
        }
        if let FinderState::Rest { started, next_rate } = self.state {
            if started.elapsed().as_secs_f64() >= REST_SECONDS {
                self.start_trial(next_rate, true, snap);
            }
            return;
        }
        if self.trial_elapsed() >= TRIAL_SECONDS {
            self.finish_trial(snap, MIN_TRIAL_DATA_SECONDS);
        }
    }

    #[cfg(test)]
    pub(crate) fn complete_rest_for_test(&mut self) {
        if let FinderState::Rest { next_rate, .. } = self.state {
            self.state = FinderState::Rest {
                started: Instant::now() - std::time::Duration::from_secs(REST_SECONDS as u64 + 1),
                next_rate,
            };
        }
    }
}
