use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Local, TimeZone, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BeatRecord {
    pub time_secs: f64,
    #[serde(default)]
    pub received_secs: f64,
    pub ibi_ms: f64,
    pub artifact: bool,
    pub hr: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SessionRecord {
    pub started_unix: i64,
    pub duration_secs: f64,
    pub mean_hr: f64,
    pub mean_score: f64,
    pub beats: usize,
    pub artifacts: usize,
    #[serde(default)]
    pub samples: Vec<BeatRecord>,
}

#[derive(Default)]
pub struct SessionBuilder {
    samples: Vec<BeatRecord>,
}

impl SessionBuilder {
    pub fn from_samples(samples: &[BeatRecord]) -> Self {
        Self {
            samples: samples.to_vec(),
        }
    }

    pub fn finish(
        self,
        started_unix: i64,
        duration_secs: f64,
        mean_hr: f64,
        mean_score: f64,
        beats: usize,
        artifacts: usize,
    ) -> SessionRecord {
        SessionRecord {
            started_unix,
            duration_secs,
            mean_hr,
            mean_score,
            beats,
            artifacts,
            samples: self.samples,
        }
    }
}

pub struct ArchiveStore;

impl ArchiveStore {
    fn path() -> PathBuf {
        #[cfg(target_os = "android")]
        {
            PathBuf::from("/sdcard/Android/data/com.emwave.resonance/files/sessions.json")
        }
        #[cfg(not(target_os = "android"))]
        {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            home.join(".emwave-resonance").join("sessions.json")
        }
    }

    pub fn load() -> Vec<SessionRecord> {
        let Ok(data) = fs::read(Self::path()) else {
            return Vec::new();
        };
        serde_json::from_slice(&data).unwrap_or_default()
    }

    pub fn append(mut sessions: Vec<SessionRecord>, record: SessionRecord) -> Vec<SessionRecord> {
        if let Some(existing) = sessions
            .iter_mut()
            .find(|s| (s.started_unix - record.started_unix).abs() <= 1)
        {
            *existing = record;
        } else {
            sessions.push(record);
        }
        sessions.sort_by_key(|s| std::cmp::Reverse(s.started_unix));
        let _ = Self::save(&sessions);
        sessions
    }

    fn save(sessions: &[SessionRecord]) -> std::io::Result<()> {
        let destination = Self::path();
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = destination.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(sessions).unwrap())?;
        fs::rename(tmp, destination)
    }

    pub fn format_date(unix: i64) -> String {
        let dt: DateTime<Utc> = Utc.timestamp_opt(unix, 0).single().unwrap_or_else(Utc::now);
        dt.with_timezone(&Local).format("%Y-%m-%d  %H:%M").to_string()
    }
}
