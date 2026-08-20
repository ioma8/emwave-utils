#[derive(Clone, Debug, Default)]
pub struct NnSeries {
    intervals: Vec<(f64, f64)>,
}

impl NnSeries {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, time_secs: f64, ibi_ms: f64) {
        self.intervals.push((time_secs, ibi_ms));
    }


    pub fn as_slice(&self) -> &[(f64, f64)] {
        &self.intervals
    }
    pub fn last(&self) -> Option<&(f64, f64)> {
        self.intervals.last()
    }

    #[cfg(test)]
    pub fn from_vec(intervals: Vec<(f64, f64)>) -> Self {
        Self { intervals }
    }
    pub fn trim_to(&mut self, capacity: usize) {
        if self.intervals.len() > capacity {
            self.intervals.drain(..self.intervals.len() - capacity);
        }
    }
}
