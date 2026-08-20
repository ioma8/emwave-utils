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

pub fn load() -> Result<Vec<SessionRecord>, String> {
    let destination = path();
    let data = match fs::read(&destination) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!("cannot read {}: {error}", destination.display()));
        }
    };
    serde_json::from_slice(&data)
        .map_err(|error| format!("cannot decode {}: {error}", destination.display()))
}

pub fn append(sessions: &mut Vec<SessionRecord>, record: SessionRecord) -> Result<(), String> {
    if let Some(existing) = sessions
        .iter_mut()
        .find(|s| (s.started_unix - record.started_unix).abs() <= 1)
    {
        *existing = record;
    } else {
        sessions.push(record);
    }
    sessions.sort_by_key(|s| std::cmp::Reverse(s.started_unix));
    save(sessions)
}

fn save(sessions: &[SessionRecord]) -> Result<(), String> {
    let destination = path();
    let parent = destination
        .parent()
        .ok_or_else(|| format!("archive path has no parent: {}", destination.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    let temporary = destination.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(sessions)
        .map_err(|error| format!("cannot encode archive: {error}"))?;
    fs::write(&temporary, data)
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    fs::rename(&temporary, &destination)
        .map_err(|error| format!("cannot replace {}: {error}", destination.display()))
}

pub fn format_date(unix: i64) -> String {
    let dt: DateTime<Utc> = Utc.timestamp_opt(unix, 0).single().unwrap_or_else(Utc::now);
    dt.with_timezone(&Local)
        .format("%Y-%m-%d  %H:%M")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_archive_keeps_every_sample() {
        let samples: Vec<_> = (0..1_000)
            .map(|index| BeatRecord {
                time_secs: index as f64,
                received_secs: index as f64,
                ibi_ms: 1_000.0,
                artifact: false,
                hr: 60.0,
            })
            .collect();
        let record = SessionRecord {
            started_unix: 1,
            duration_secs: 1_000.0,
            mean_hr: 60.0,
            mean_score: 50.0,
            beats: 1_000,
            artifacts: 0,
            samples,
        };
        let decoded: SessionRecord =
            serde_json::from_slice(&serde_json::to_vec(&record).unwrap()).unwrap();

        assert_eq!(decoded.samples.len(), 1_000);
        assert_eq!(decoded.samples.first().unwrap().time_secs, 0.0);
        assert_eq!(decoded.samples.last().unwrap().time_secs, 999.0);
    }
}
