#![cfg_attr(target_os = "android", no_main)]
//! emWave2 resonance trainer: paced breathing + live HR/HRV/resonance.
//!
//! Reader thread talks HID and publishes a snapshot; the egui thread renders.

mod analysis;
mod archive;
mod emwave;
mod metrics;
mod protocol;
mod runtime;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use archive::{ArchiveStore, SessionBuilder};
use eframe::egui;
use analysis::NnSeries;
use metrics::{HrvMetrics, Resonance};
use runtime::{FinderRuntime, FINDER_RATES, MIN_TRIAL_DATA_SECONDS, REST_SECONDS, TRIAL_SECONDS};
const WINDOW_BEATS: usize = 240;
const GRAPH_BEATS: usize = 120;
const DEFAULT_PACER_RATE: f64 = 6.0;
const INHALE_FRACTION: f64 = 0.4;

#[derive(Clone, Default)]
struct Snapshot {
    connected: bool,
    status: String,
    hr: f64,
    ibi: f64,
    beats: usize,
    artifacts: usize,
    elapsed: f64,
    mean_hr: f64,
    mean_score: f64,
    session_started_unix: i64,
    hrv: Option<HrvMetrics>,
    res: Option<Resonance>,
    analysis_received: NnSeries,
    series: Vec<f64>,
    sessions: Vec<archive::SessionRecord>,
}

// --------------------------------------------------------------------------
// Stream parsing & accumulation
// --------------------------------------------------------------------------

struct Beat {
    ibi_ms: f64,
    artifact: bool,
    hr: f64,
}


/// Parse one `<2 I=NNN R=F H=NN >` record line.
fn parse_ibi(line: &[u8]) -> Option<Beat> {
    let i = line
        .windows(b"<2 I=".len())
        .position(|window| window == b"<2 I=")? + 5;
    let mut j = i;
    let mut ibi = 0.0;
    while j < line.len() && line[j].is_ascii_digit() {
        ibi = ibi * 10.0 + (line[j] - b'0') as f64;
        j += 1;
    }
    if ibi <= 0.0 {
        return None;
    }
    let artifact = line
        .windows(2)
        .position(|window| window == b"R=")
        .and_then(|r| line.get(r + 2))
        .map(|&b| b == b'T')
        .unwrap_or(false);
    let mut hr = 0.0;
    if let Some(h) = line
        .windows(2)
        .position(|window| window == b"H=")
    {
        let mut k = h + 2;
        while k < line.len() && line[k].is_ascii_digit() {
            hr = hr * 10.0 + (line[k] - b'0') as f64;
            k += 1;
        }
    }
    Some(Beat { ibi_ms: ibi, artifact, hr })
}

/// Line reassembler: records end with `\r` and may split across reports.
struct BeatParser {
    buf: Vec<u8>,
}

impl BeatParser {
    fn new() -> Self {
        BeatParser { buf: Vec::new() }
    }
    fn feed(&mut self, payload: &[u8]) {
        self.buf.extend_from_slice(payload);
    }
    fn next(&mut self) -> Option<Beat> {
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\r') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            if let Some(b) = parse_ibi(&line) {
                return Some(b);
            }
        }
        None
}
}

struct HeartStream {
    ibis: NnSeries,
    analysis_received: NnSeries,
    raw: Vec<f64>,
    last_hr: f64,
    last_ibi: f64,
    artifacts: usize,
    clean_beats_total: usize,
    beat_time: f64,
    started: Instant,
    started_unix: i64,
    hr_sum: f64,
    hr_count: usize,
    score_sum: f64,
    score_count: usize,
    sessions: Vec<archive::SessionRecord>,
}

impl HeartStream {
    fn new(sessions: Vec<archive::SessionRecord>) -> Self {
        HeartStream {
            ibis: NnSeries::new(),
            analysis_received: NnSeries::new(),
            raw: Vec::new(),
            last_hr: 0.0,
            last_ibi: 0.0,
            artifacts: 0,
            clean_beats_total: 0,
            beat_time: 0.0,
            started: Instant::now(),
            started_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or_default(),
            hr_sum: 0.0,
            hr_count: 0,
            score_sum: 0.0,
            score_count: 0,
            sessions,
        }
    }

    fn ingest(&mut self, b: Beat) {
        self.last_ibi = b.ibi_ms;
        if !b.artifact && b.ibi_ms > 0.0 {
            let derived_hr = 60_000.0 / b.ibi_ms;
            self.last_hr = derived_hr;
            self.hr_sum += derived_hr;
            self.hr_count += 1;
        }
        self.beat_time += b.ibi_ms / 1000.0;
        if b.artifact {
            self.artifacts += 1;
        } else {
            self.clean_beats_total += 1;
            self.ibis.push(self.beat_time, b.ibi_ms);
            self.ibis.trim_to(WINDOW_BEATS);
            if let Some(res) = metrics::resonance(&self.ibis) {
                self.score_sum += res.score;
                self.score_count += 1;
            }
        }
        self.raw.push(b.ibi_ms);
        if self.raw.len() > GRAPH_BEATS {
            self.raw.drain(..self.raw.len() - GRAPH_BEATS);
        }
    }

    fn snapshot(&self, status: &str, connected: bool) -> Snapshot {
        Snapshot {
            connected,
            status: status.to_string(),
            hr: self.last_hr,
            ibi: self.last_ibi,
            beats: self.clean_beats_total,
            artifacts: self.artifacts,
            elapsed: self.started.elapsed().as_secs_f64(),
            mean_hr: if self.hr_count > 0 {
                self.hr_sum / self.hr_count as f64
            } else {
                0.0
            },
            mean_score: if self.score_count > 0 {
                self.score_sum / self.score_count as f64
            } else {
                0.0
            },
            session_started_unix: self.started_unix,
            hrv: metrics::hrv_metrics(&self.ibis),
            res: metrics::resonance(&self.ibis),
            analysis_received: self.analysis_received.clone(),
            series: self.raw.clone(),
            sessions: self.sessions.clone(),
        }
    }
}

// --------------------------------------------------------------------------
// Reader thread
fn diagnostics_path() -> &'static str {
    #[cfg(target_os = "android")]
    {
        "/sdcard/Android/data/com.emwave.resonance/files/emwave.log"
    }
    #[cfg(not(target_os = "android"))]
    {
        "/tmp/emwave-resonance.log"
    }
}

fn persist_snapshot(snap: &mut Snapshot, samples: &[archive::BeatRecord]) {
    if snap.beats == 0 || snap.session_started_unix <= 0 {
        return;
    }
    let record = SessionBuilder::from_samples(samples).finish(
        snap.session_started_unix,
        snap.elapsed,
        snap.mean_hr,
        snap.mean_score,
        snap.beats,
        snap.artifacts,
    );
    snap.sessions = ArchiveStore::append(snap.sessions.clone(), record);
}

fn diagnostic(message: impl AsRef<str>) {
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(diagnostics_path())
    {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let _ = writeln!(file, "[{now}] {}", message.as_ref());
    }
}

// --------------------------------------------------------------------------

#[allow(unused_mut)]
fn reader_loop(
    shared: Arc<Mutex<Snapshot>>,
    session_samples: Arc<Mutex<Vec<archive::BeatRecord>>>,
    platform: emwave::AndroidContext,
) {
    diagnostic("reader thread started");
    loop {
        diagnostic("opening emWave2");
        match emwave::Device::open_and_start(platform) {
            Ok(mut dev) => {
                let mut stream = HeartStream::new(ArchiveStore::load());
                let reader_started = Instant::now();
                session_samples.lock().unwrap().clear();
                let mut parser = BeatParser::new();
                *shared.lock().unwrap() = stream.snapshot("connected", true);
                loop {
                    match dev.read_report(150) {
                        Ok(Some(rep)) => {
                            if rep[0] == 0x75 {
                                parser.feed(&rep[4..4 + rep[3] as usize]);
                                while let Some(b) = parser.next() {
                                    let received_secs = reader_started.elapsed().as_secs_f64();
                                    let sample = (b.ibi_ms, b.artifact, b.hr);
                                    stream.ingest(b);
                                    if !sample.1 {
                                        stream.analysis_received.push(received_secs, sample.0);
                                        stream.analysis_received.trim_to(WINDOW_BEATS);
                                        }
                                    session_samples.lock().unwrap().push(archive::BeatRecord {
                                        time_secs: stream.beat_time,
                                        received_secs,
                                        ibi_ms: sample.0,
                                        artifact: sample.1,
                                        hr: sample.2,
                                    });
                                }
                                *shared.lock().unwrap() = stream.snapshot("connected", true);
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            diagnostic(format!("HID read error: {e}"));
                            let mut snap =
                                stream.snapshot(&format!("read error: {e}"), false);
                            let samples = session_samples.lock().unwrap().clone();
                            persist_snapshot(&mut snap, &samples);
                            *shared.lock().unwrap() = snap;
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                diagnostic(format!("USB/HID open error: {e}"));
                let mut snap = shared.lock().unwrap().clone();
                snap.connected = false;
                snap.status = e;
                *shared.lock().unwrap() = snap;
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

// --------------------------------------------------------------------------
// egui app
// --------------------------------------------------------------------------

const BG: egui::Color32 = egui::Color32::from_rgb(9, 13, 18);
const CARD: egui::Color32 = egui::Color32::from_rgb(16, 23, 31);
const CARD_ALT: egui::Color32 = egui::Color32::from_rgb(20, 29, 39);
const CHART_BG: egui::Color32 = egui::Color32::from_rgb(11, 17, 23);
const BORDER: egui::Color32 = egui::Color32::from_rgb(36, 49, 63);
const TEXT: egui::Color32 = egui::Color32::from_rgb(237, 242, 247);
const MUTED: egui::Color32 = egui::Color32::from_rgb(137, 152, 168);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(94, 225, 190);
const BLUE: egui::Color32 = egui::Color32::from_rgb(105, 167, 255);
const AMBER: egui::Color32 = egui::Color32::from_rgb(246, 190, 91);
const RED: egui::Color32 = egui::Color32::from_rgb(242, 107, 102);

struct App {
    shared: Arc<Mutex<Snapshot>>,
    session_samples: Arc<Mutex<Vec<archive::BeatRecord>>>,
    started: Instant,
    pacer_enabled: bool,
    pacer_rate: f64,
    view: View,
    finder: FinderRuntime,
    selected_session: Option<usize>,
}

impl App {
    fn new(
        cc: &eframe::CreationContext<'_>,
        shared: Arc<Mutex<Snapshot>>,
        session_samples: Arc<Mutex<Vec<archive::BeatRecord>>>,
    ) -> Self {
        configure_style(&cc.egui_ctx);
        shared.lock().unwrap().sessions = ArchiveStore::load();
        Self {
            shared,
            session_samples,
            started: Instant::now(),
            pacer_enabled: true,
            pacer_rate: DEFAULT_PACER_RATE,
            view: View::Train,
            finder: FinderRuntime::default(),
            selected_session: None,
        }
    }

    fn pacer(&self) -> (&'static str, f64, f64) {
        let cycle = 60.0 / self.pacer_rate.max(1.0);
        let inhale = cycle * INHALE_FRACTION;
        let pos = self.started.elapsed().as_secs_f64() % cycle;
        if pos < inhale {
            ("INHALE", pos / inhale, pos / inhale)
        } else {
            let progress = (pos - inhale) / (cycle - inhale);
            ("EXHALE", progress, 1.0 - progress)
        }
    }

}

fn configure_style(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    let visuals = &mut style.visuals;
    visuals.panel_fill = BG;
    visuals.window_fill = BG;
    visuals.extreme_bg_color = CHART_BG;
    visuals.faint_bg_color = CARD_ALT;
    visuals.override_text_color = Some(TEXT);
    visuals.selection.bg_fill = egui::Color32::from_rgb(43, 112, 99);
    visuals.hyperlink_color = ACCENT;
    visuals.widgets.noninteractive.bg_fill = CARD;
    visuals.widgets.inactive.bg_fill = CARD_ALT;
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(29, 41, 53);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(37, 54, 67);
    style.spacing.item_spacing = egui::vec2(12.0, 12.0);
    style.spacing.button_padding = egui::vec2(14.0, 9.0);
    style.text_styles.insert(
        egui::TextStyle::Heading,
        egui::FontId::proportional(27.0),
    );
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
    ctx.set_style_of(egui::Theme::Dark, style);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Train,
    Finder,
    Sessions,
}

fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(16)
        .inner_margin(egui::Margin::same(16))
}

fn state_color(state: &str) -> egui::Color32 {
    match state {
        "RESONANT" => ACCENT,
        "PARASYMPATHETIC" => BLUE,
        "STRESS" => RED,
        "BUILDING" => MUTED,
        _ => AMBER,
    }
}

fn pill(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    egui::Frame::new()
        .fill(color)
        .corner_radius(20)
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(text)
                    .size(11.0)
                    .strong()
                    .color(egui::Color32::from_rgb(7, 17, 21)),
            );
        });
}

fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(11.0)
            .strong()
            .color(MUTED),
    );
}


fn duration(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    format!("{:02}:{:02}", total / 60, total % 60)
}
fn compact_stat(ui: &mut egui::Ui, label: &str, value: &str, unit: &str) {
    ui.vertical(|ui| {
        section_label(ui, label);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(value).size(20.0).strong().color(TEXT));
            ui.label(egui::RichText::new(unit).size(10.0).color(MUTED));
        });
    });
}
fn mobile_dashboard(ui: &mut egui::Ui, snap: &Snapshot, app: &mut App) {
    let state = snap.res.map(|r| r.state).unwrap_or("BUILDING");
    let state_tint = state_color(state);
    let pacer_live = snap.connected && app.pacer_enabled;
    let (phase, progress, amount) = if pacer_live {
        app.pacer()
    } else {
        ("PAUSED", 0.0, 0.0)
    };
    let tint = if !pacer_live {
        MUTED
    } else if phase == "INHALE" {
        ACCENT
    } else {
        BLUE
    };

    card().inner_margin(egui::Margin::same(10)).show(ui, |ui| {
        ui.horizontal(|ui| {
            section_label(ui, "BREATH PACER");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(snap.connected, |ui| {
                    ui.checkbox(&mut app.pacer_enabled, "Enabled");
                });
            });
        });
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(96.0, 96.0), egui::Sense::hover());
            let center = rect.center();
            let painter = ui.painter_at(rect);
            let radius = 24.0 + 25.0 * amount as f32;
            painter.circle_filled(center, 46.0, CHART_BG);
            painter.circle_stroke(center, 46.0, egui::Stroke::new(1.0, BORDER));
            painter.circle_filled(
                center,
                radius,
                egui::Color32::from_rgba_premultiplied(tint.r(), tint.g(), tint.b(), 52),
            );
            painter.circle_stroke(center, radius, egui::Stroke::new(2.0, tint));
            painter.text(
                center,
                egui::Align2::CENTER_CENTER,
                phase,
                egui::FontId::proportional(13.0),
                TEXT,
            );
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(format!("{:.1} breaths/min", app.pacer_rate))
                        .size(16.0)
                        .strong()
                        .color(TEXT),
                );
                ui.label(
                    egui::RichText::new(if !snap.connected {
                        "Connect emWave2 to start the session"
                    } else if app.pacer_enabled {
                        "Follow the circle"
                    } else {
                        "Measurements continue"
                    })
                    .size(11.0)
                    .color(MUTED),
                );
                ui.add(
                    egui::ProgressBar::new(progress as f32)
                        .desired_width(ui.available_width())
                        .fill(tint)
                        .text(""),
                );
            });
        });
    });

    card()
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_min_height(88.0);
            ui.horizontal(|ui| {
                section_label(ui, "CARDIAC ALIGNMENT");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    pill(ui, state, state_tint);
                });
            });
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                let score = snap.res.map(|r| r.score).unwrap_or(0.0);
                ui.label(
                    egui::RichText::new(if snap.res.is_some() {
                        format!("{score:.0}")
                    } else {
                        "—".to_owned()
                    })
                    .size(32.0)
                    .strong()
                    .color(TEXT),
                );
                if snap.res.is_some() {
                    ui.label(egui::RichText::new("%").size(12.0).color(MUTED));
                }
                ui.add(
                    egui::ProgressBar::new((score / 100.0) as f32)
                        .desired_width(ui.available_width())
                        .corner_radius(6)
                        .fill(state_tint)
                        .text(""),
                );
            });
            ui.label(
                egui::RichText::new(
                    snap.res
                        .map(|r| format!("peak {:.1} bpm  ·  LF {:.0}%  ·  HF {:.0}%", r.bpm, r.lf_nu, r.hf_nu))
                        .unwrap_or_else(|| "Collecting clean beats…".to_owned()),
                )
                .size(10.0)
                .color(MUTED),
            );
        });

    ui.add_space(6.0);
    card().inner_margin(egui::Margin::same(10)).show(ui, |ui| {
        ui.columns(2, |cols| {
            let hr = if snap.hr > 0.0 {
                format!("{:.0}", snap.hr)
            } else {
                "—".to_owned()
            };
            let ibi = if snap.ibi > 0.0 {
                format!("{:.0}", snap.ibi)
            } else {
                "—".to_owned()
            };
            compact_stat(&mut cols[0], "HEART RATE", &hr, "bpm");
            compact_stat(&mut cols[1], "INTER-BEAT", &ibi, "ms");
        });
    });

    ui.add_space(6.0);
    card().inner_margin(egui::Margin::same(10)).show(ui, |ui| {
        ui.horizontal(|ui| {
            section_label(ui, "HEART RHYTHM");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("{} beats · {} artifacts", snap.beats, snap.artifacts))
                        .size(10.0)
                        .color(MUTED),
                );
            });
        });
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 92.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 8.0, CHART_BG);
        if snap.series.len() >= 2 {
            let lo = snap.series.iter().cloned().fold(f64::INFINITY, f64::min) - 25.0;
            let hi = snap.series.iter().cloned().fold(f64::NEG_INFINITY, f64::max) + 25.0;
            let range = (hi - lo).max(80.0);
            let points: Vec<egui::Pos2> = snap
                .series
                .iter()
                .enumerate()
                .map(|(i, &value)| {
                    let x = rect.left()
                        + 6.0
                        + i as f32 / (snap.series.len() - 1) as f32 * (rect.width() - 12.0);
                    let y = rect.bottom()
                        - 6.0
                        - ((value - lo) / range) as f32 * (rect.height() - 12.0);
                    egui::pos2(x, y)
                })
                .collect();
            painter.add(egui::Shape::line(points, egui::Stroke::new(1.5, ACCENT)));
        } else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Waiting for clean beats…",
                egui::FontId::proportional(11.0),
                MUTED,
            );
        }
        ui.columns(4, |cols| {
            let rmssd = snap.hrv.map(|h| format!("{:.1}", h.rmssd)).unwrap_or_else(|| "—".to_owned());
            let sdnn = snap.hrv.map(|h| format!("{:.1}", h.sdnn)).unwrap_or_else(|| "—".to_owned());
            let pnn50 = snap.hrv.map(|h| format!("{:.1}", h.pnn50)).unwrap_or_else(|| "—".to_owned());
            let peak = snap.res.map(|r| format!("{:.1}", r.bpm)).unwrap_or_else(|| "—".to_owned());
            compact_stat(&mut cols[0], "RMSSD", &rmssd, "ms");
            compact_stat(&mut cols[1], "SDNN", &sdnn, "ms");
            compact_stat(&mut cols[2], "pNN50", &pnn50, "%");
            compact_stat(&mut cols[3], "PEAK", &peak, "bpm");
        });
        ui.label(
            egui::RichText::new(snap.status.as_str())
                .size(9.0)
                .color(MUTED),
        );
    });
}


fn finder_pacer(ui: &mut egui::Ui, app: &App) {
    if !app.finder.active {
        return;
    }
    if app.finder.resting {
        let elapsed = app
            .finder
            .rest_started
            .map(|started| started.elapsed().as_secs_f64())
            .unwrap_or(0.0)
            .min(REST_SECONDS);
        card().show(ui, |ui| {
            section_label(ui, "REST BETWEEN TRIALS");
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("Breathe naturally; do not follow the pacer.")
                    .size(16.0)
                    .color(TEXT),
            );
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(format!("{:.0}s remaining", REST_SECONDS - elapsed))
                    .size(28.0)
                    .strong()
                    .color(TEXT),
            );
            ui.add(
                egui::ProgressBar::new((elapsed / REST_SECONDS) as f32)
                    .desired_width(ui.available_width())
                    .fill(MUTED)
                    .text(format!("next: {:.1} breaths/min", app.pacer_rate)),
            );
        });
        return;
    }
    let (phase, progress, amount) = app.pacer();
    let tint = if phase == "INHALE" { ACCENT } else { BLUE };
    card().show(ui, |ui| {
        ui.horizontal(|ui| {
            section_label(ui, "BREATH PACER");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                pill(
                    ui,
                    &format!("{:.1} breaths/min", app.pacer_rate),
                    tint,
                );
            });
        });
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 180.0), egui::Sense::hover());
        let center = rect.center();
        let radius = 34.0 + 38.0 * amount as f32;
        let painter = ui.painter_at(rect);
        painter.circle_filled(center, 76.0, CHART_BG);
        painter.circle_stroke(center, 76.0, egui::Stroke::new(1.0, BORDER));
        painter.circle_filled(
            center,
            radius,
            egui::Color32::from_rgba_premultiplied(tint.r(), tint.g(), tint.b(), 52),
        );
        painter.circle_stroke(center, radius, egui::Stroke::new(3.0, tint));
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            phase,
            egui::FontId::proportional(19.0),
            TEXT,
        );
        ui.add(
            egui::ProgressBar::new(progress as f32)
                .desired_width(ui.available_width())
                .fill(tint)
                .text("follow the circle gently"),
        );
    });
}

fn resonance_finder(ui: &mut egui::Ui, snap: &Snapshot, app: &mut App) {
    let current_rate = FINDER_RATES[app.finder.rate_index];
    let resting = app.finder.resting;
    let period = if resting { REST_SECONDS } else { TRIAL_SECONDS };
    let elapsed = if resting {
        app.finder
            .rest_started
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(0.0)
    } else {
        app.finder
            .started
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(0.0)
    }
    .min(period);

    card()
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new("Find your cardiac candidate")
                            .size(20.0)
                            .strong()
                            .color(TEXT),
                    );
                    ui.label(
                        egui::RichText::new("PPG response heuristic · respiration not measured")
                            .size(10.0)
                            .color(MUTED),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    pill(ui, "ASSESSMENT", BLUE);
                });
            });
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                for text in [
                    "5 rates",
                    "2 min trial",
                    "2 min natural rest",
                    "6.5 → 4.5 bpm",
                ] {
                    egui::Frame::new()
                        .fill(CARD_ALT)
                        .corner_radius(8)
                        .inner_margin(egui::Margin::symmetric(7, 4))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(text).size(10.0).color(MUTED));
                        });
                }
            });
        });

    ui.add_space(10.0);
    finder_pacer(ui, app);
    ui.add_space(10.0);
    if app.finder.active {
        card().show(ui, |ui| {
            ui.horizontal(|ui| {
                section_label(
                    ui,
                    if resting {
                        "REST BETWEEN TRIALS"
                    } else if app.finder.run_all {
                        "SEQUENTIAL TRIALS"
                    } else {
                        "TRIAL IN PROGRESS"
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    pill(
                        ui,
                        &format!("{current_rate:.1} breaths/min"),
                        ACCENT,
                    );
                });
            });
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(format!("{:.0}s remaining", period - elapsed))
                    .size(28.0)
                    .strong()
                    .color(TEXT),
            );
            if app.finder.run_all {
                ui.label(
                    egui::RichText::new(format!(
                        "Trial {} of {}",
                        app.finder.rate_index + 1,
                        FINDER_RATES.len()
                    ))
                    .size(13.0)
                    .color(MUTED),
                );
            }
            ui.add(
                egui::ProgressBar::new((elapsed / period) as f32)
                    .desired_width(ui.available_width())
                    .fill(if resting { MUTED } else { ACCENT })
                    .text(if resting {
                        "breathe naturally"
                    } else {
                        "breathe with the circle"
                    }),
            );
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(if snap.connected {
                    "Collecting clean heart beats…"
                } else {
                    "Device disconnected — reconnect before continuing"
                })
                .size(12.0)
                .color(if snap.connected { MUTED } else { RED }),
            );
        });
    } else {
        card().show(ui, |ui| {
            section_label(ui, "FIND YOUR RATE");
            if app.finder.interrupted {
                ui.label(
                    egui::RichText::new(
                        "Previous run interrupted by USB disconnect; incomplete trial was not scored.",
                    )
                    .size(12.0)
                    .color(RED),
                );
                ui.add_space(6.0);
            }
            ui.add_space(8.0);
            if ui
                .add_enabled(
                    snap.connected,
                    egui::Button::new("RUN 5 TRIALS · ABOUT 18 MINUTES"),
                )
                .clicked()
            {
                app.finder.results.clear();
                app.finder.start_trial(0, true, snap);
            }
            ui.label(
                egui::RichText::new(if snap.connected {
                    "Runs 6.5, 6.0, 5.5, 5.0, and 4.5 breaths/min with rests."
                } else {
                    "Connect the emWave2 before starting."
                })
                .size(12.0)
                .color(if snap.connected { MUTED } else { RED }),
            );
            ui.add_space(12.0);
            section_label(ui, "OR RUN ONE TRIAL");
            ui.horizontal_wrapped(|ui| {
                for (index, &rate) in FINDER_RATES.iter().enumerate() {
                    let completed = app.finder.results.iter().any(|r| (r.rate - rate).abs() < 0.01);
                    let label = if completed {
                        format!("✓ {rate:.1}")
                    } else {
                        format!("{rate:.1}")
                    };
                    if ui
                        .add_enabled(snap.connected, egui::Button::new(label))
                        .clicked()
                    {
                        app.finder.start_trial(index, false, snap);
                    }
                }
            });
            ui.label(
                egui::RichText::new("Each trial lasts 2 minutes; rests last 2 minutes.")
                    .size(12.0)
                    .color(MUTED),
            );
        });
    }

    if !app.finder.results.is_empty() {
        ui.add_space(10.0);
        card().show(ui, |ui| {
            section_label(ui, "TARGET-FREQUENCY RESULTS");
            ui.add_space(6.0);
            let mut ranked = app.finder.results.clone();
            ranked.sort_by(|a, b| {
                let a_reliable = a.alignment.is_some_and(|value| {
                    value.reliable && value.span_s >= MIN_TRIAL_DATA_SECONDS
                });
                let b_reliable = b.alignment.is_some_and(|value| {
                    value.reliable && value.span_s >= MIN_TRIAL_DATA_SECONDS
                });
                b_reliable.cmp(&a_reliable).then_with(|| {
                    let a_score = a.alignment.map(|value| value.score).unwrap_or(-1.0);
                    let b_score = b.alignment.map(|value| value.score).unwrap_or(-1.0);
                    b_score.partial_cmp(&a_score).unwrap()
                })
            });
            let best_rate = ranked
                .iter()
                .find(|result| {
                    result
                        .alignment
                        .is_some_and(|value| value.reliable && value.span_s >= MIN_TRIAL_DATA_SECONDS)
                })
                .map(|result| result.rate);
            for result in ranked {
                let (status, tint, detail) = match result.alignment {
                    Some(value) if value.span_s < MIN_TRIAL_DATA_SECONDS => (
                        "PARTIAL",
                        AMBER,
                        format!(
                            "score {:.0} · peak {:.1} · Δ{:.1} · target {:.0}% LF {:.0}% · n {:.0}s",
                            value.score,
                            value.peak_bpm,
                            value.mismatch_bpm,
                            value.target_peak_ratio * 100.0,
                            value.lf_nu,
                            value.span_s
                        ),
                    ),
                    Some(value) if Some(result.rate) == best_rate => (
                        "BEST CANDIDATE",
                        ACCENT,
                        format!(
                            "score {:.0} · peak {:.1} · Δ{:.1} · target {:.0}% LF {:.0}% · n {:.0}s",
                            value.score,
                            value.peak_bpm,
                            value.mismatch_bpm,
                            value.target_peak_ratio * 100.0,
                            value.lf_nu,
                            value.span_s
                        ),
                    ),
                    Some(value) if value.reliable => (
                        "CANDIDATE",
                        BLUE,
                        format!(
                            "score {:.0} · peak {:.1} · Δ{:.1} · target {:.0}% LF {:.0}% · n {:.0}s",
                            value.score,
                            value.peak_bpm,
                            value.mismatch_bpm,
                            value.target_peak_ratio * 100.0,
                            value.lf_nu,
                            value.span_s
                        ),
                    ),
                    Some(value) => (
                        "NO MATCH",
                        AMBER,
                        format!(
                            "score {:.0} · peak {:.1} · Δ{:.1} · target {:.0}% LF {:.0}% · n {:.0}s",
                            value.score,
                            value.peak_bpm,
                            value.mismatch_bpm,
                            value.target_peak_ratio * 100.0,
                            value.lf_nu,
                            value.span_s
                        ),
                    ),
                    None => ("NO DATA", RED, "insufficient clean trial data".to_owned()),
                };
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(status)
                                .size(10.0)
                                .strong()
                                .color(tint),
                        );
                        ui.label(
                            egui::RichText::new(format!("{:.1} breaths/min", result.rate))
                                .size(14.0)
                                .color(TEXT),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(detail).size(11.0).color(MUTED));
                    });
                });
                ui.add_space(4.0);
            }
            ui.label(
                egui::RichText::new(match best_rate {
                    Some(rate) => format!(
                        "Best cardiac candidate: {rate:.1} breaths/min. Confirm it with respiration monitoring or a longer session."
                    ),
                    None => "No reliable cardiac candidate. Repeat with verified breathing and a stable sensor.".to_owned(),
                })
                .size(12.0)
                .color(MUTED),
            );
            ui.label(
                egui::RichText::new(
                    "A candidate requires a peak within 0.5 bpm of the paced rate and score ≥35; this is not phase-verified without respiration.",
                )
                .size(11.0)
                .color(MUTED),
            );
            ui.label(
                egui::RichText::new(
                    "Score measures response at the tested rate; MATCH additionally requires the dominant LF peak to align.",
                )
                .size(11.0)
                .color(MUTED),
            );
        });
    }
}

fn session_graph(ui: &mut egui::Ui, session: &archive::SessionRecord) {
    card().show(ui, |ui| {
        ui.horizontal(|ui| {
            section_label(ui, "ARCHIVED HEART RHYTHM");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("{} samples", session.samples.len()))
                        .size(11.0)
                        .color(MUTED),
                );
            });
        });
        if session.samples.len() < 2 {
            ui.label(
                egui::RichText::new("This older session has no archived beat waveform.")
                    .size(12.0)
                    .color(AMBER),
            );
            return;
        }
        let (lo, hi) = session
            .samples
            .iter()
            .map(|sample| sample.ibi_ms)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), ibi| {
                (lo.min(ibi), hi.max(ibi))
            });
        let range = (hi - lo).max(100.0);
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 220.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 11.0, CHART_BG);
        let mut clean_points = Vec::new();
        for (index, sample) in session.samples.iter().enumerate() {
            let x = rect.left()
                + 8.0
                + index as f32 / (session.samples.len() - 1) as f32 * (rect.width() - 16.0);
            let y = rect.bottom()
                - 8.0
                - ((sample.ibi_ms - lo) / range) as f32 * (rect.height() - 16.0);
            if sample.artifact {
                painter.circle_filled(egui::pos2(x, y), 3.5, RED);
            } else {
                clean_points.push(egui::pos2(x, y));
            }
        }
        if clean_points.len() >= 2 {
            painter.add(egui::Shape::line(
                clean_points,
                egui::Stroke::new(2.0, ACCENT),
            ));
        }
        painter.text(
            rect.left_top() + egui::vec2(10.0, 10.0),
            egui::Align2::LEFT_TOP,
            format!("{hi:.0} ms"),
            egui::FontId::monospace(10.0),
            MUTED,
        );
        painter.text(
            rect.left_bottom() + egui::vec2(10.0, -10.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{lo:.0} ms"),
            egui::FontId::monospace(10.0),
            MUTED,
        );
        ui.label(
            egui::RichText::new("Green: clean NN intervals · red: artifact-marked intervals")
                .size(11.0)
                .color(MUTED),
        );
    });
}

fn session_history(
    ui: &mut egui::Ui,
    sessions: &[archive::SessionRecord],
    selected_session: &mut Option<usize>,
) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Previous sessions")
                .size(24.0)
                .strong()
                .color(TEXT),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("{} recorded", sessions.len()))
                    .size(12.0)
                    .color(MUTED),
            );
        });
    });
    ui.add_space(12.0);

    if sessions.is_empty() {
        card().show(ui, |ui| {
            ui.label(
                egui::RichText::new("No sessions recorded yet")
                    .size(17.0)
                    .color(TEXT),
            );
            ui.label(
                egui::RichText::new("Run a training session and it will appear here.")
                    .size(13.0)
                    .color(MUTED),
            );
        });
        return;
    }

    for (index, session) in sessions.iter().enumerate() {
        card().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("#{}", sessions.len() - index))
                        .size(12.0)
                        .color(MUTED),
                );
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(archive::ArchiveStore::format_date(session.started_unix))
                            .size(15.0)
                            .strong()
                            .color(TEXT),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{} beats  ·  {} artifacts",
                            session.beats, session.artifacts
                        ))
                        .size(12.0)
                        .color(MUTED),
                    );
                });
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(format!("{:.0}%", session.mean_score))
                                    .size(22.0)
                                    .strong()
                                    .color(state_color(if session.mean_score >= 75.0 {
                                        "RESONANT"
                                    } else {
                                        "BALANCED"
                                    })),
                            );
                            ui.label(
                                egui::RichText::new(format!(
                                    "{:.0} bpm  ·  {:.0}s",
                                    session.mean_hr, session.duration_secs
                                ))
                                .size(11.0)
                                .color(MUTED),
                            );
                        });
                    },
                );
            });
            if ui
                .button(if *selected_session == Some(index) {
                    "VIEWING GRAPH"
                } else if session.samples.is_empty() {
                    "NO ARCHIVE"
                } else {
                    "VIEW GRAPH"
                })
                .clicked()
                && !session.samples.is_empty()
            {
                *selected_session = Some(index);
            }
        });
        ui.add_space(8.0);
    }
    if let Some(index) = *selected_session {
        if let Some(session) = sessions.get(index) {
            ui.add_space(8.0);
            session_graph(ui, session);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequential_finder_runs_every_rate_then_stops() {
        let mut app = App {
            shared: Arc::new(Mutex::new(Snapshot::default())),
            session_samples: Arc::new(Mutex::new(Vec::new())),
            started: Instant::now(),
            pacer_enabled: true,
            pacer_rate: DEFAULT_PACER_RATE,
            view: View::Finder,
            finder: FinderRuntime::default(),
            selected_session: None,
        };
        let snap = Snapshot::default();

        app.finder.start_trial(0, true, &snap);
        for expected in 1..FINDER_RATES.len() {
            app.finder.finish_trial(&snap, MIN_TRIAL_DATA_SECONDS);
            assert!(app.finder.active);
            assert_eq!(app.finder.rate_index, expected);
        }
        app.finder.finish_trial(&snap, MIN_TRIAL_DATA_SECONDS);

        assert!(!app.finder.active);
    }

    #[test]
    fn sequential_finder_inserts_natural_breathing_rest() {
        let mut app = App {
            shared: Arc::new(Mutex::new(Snapshot::default())),
            session_samples: Arc::new(Mutex::new(Vec::new())),
            started: Instant::now(),
            pacer_enabled: true,
            pacer_rate: DEFAULT_PACER_RATE,
            view: View::Finder,
            finder: FinderRuntime::default(),
            selected_session: None,
        };
        let snap = Snapshot {
            connected: true,
            ..Default::default()
        };
        app.finder.start_trial(0, true, &snap);
        app.finder.finish_trial(&snap, MIN_TRIAL_DATA_SECONDS);
        assert!(app.finder.active);
        assert!(app.finder.resting);
        assert_eq!(app.finder.rate_index, 1);
        app.finder.rest_started =
            Some(Instant::now() - Duration::from_secs(REST_SECONDS as u64 + 1));
        app.finder.update(&snap);
        assert!(!app.finder.resting);
        assert_eq!(app.finder.rate_index, 1);
        assert!(app.finder.started.is_some());
    }

    #[test]
    fn disconnect_interrupts_trial_without_scoring() {
        let mut app = App {
            shared: Arc::new(Mutex::new(Snapshot::default())),
            session_samples: Arc::new(Mutex::new(Vec::new())),
            started: Instant::now(),
            pacer_enabled: true,
            pacer_rate: DEFAULT_PACER_RATE,
            view: View::Finder,
            finder: FinderRuntime::default(),
            selected_session: None,
        };
        let connected = Snapshot::default();
        app.finder.start_trial(0, true, &connected);
        let disconnected = Snapshot {
            connected: false,
            ..connected
        };
        app.finder.update(&disconnected);
        assert!(!app.finder.active);
        assert!(app.finder.interrupted);
        assert!(app.finder.results.is_empty());
    }

    #[test]
    fn finder_uses_receipt_time_for_trial_boundaries() {
        let mut app = App {
            shared: Arc::new(Mutex::new(Snapshot::default())),
            session_samples: Arc::new(Mutex::new(Vec::new())),
            started: Instant::now(),
            pacer_enabled: true,
            pacer_rate: DEFAULT_PACER_RATE,
            view: View::Finder,
            finder: FinderRuntime::default(),
            selected_session: None,
        };
        let snap = Snapshot {
            analysis_received: NnSeries::from_vec(vec![(12.5, 800.0)]),
            ..Default::default()
        };
        app.finder.start_trial(0, false, &snap);
        assert_eq!(app.finder.data_started_s, Some(12.5));
    }

    #[test]
    fn artifact_heart_rate_does_not_change_session_mean() {
        let mut stream = HeartStream::new(Vec::new());
        stream.ingest(Beat {
            ibi_ms: 1000.0,
            artifact: false,
            hr: 120.0,
        });
        stream.ingest(Beat {
            ibi_ms: 1000.0,
            artifact: true,
            hr: 120.0,
        });
        stream.ingest(Beat {
            ibi_ms: 1000.0,
            artifact: false,
            hr: 60.0,
        });
        assert_eq!(stream.snapshot("test", true).mean_hr, 60.0);
    }
    #[test]
    fn archive_beat_count_exceeds_analysis_window() {
        let mut stream = HeartStream::new(Vec::new());
        for _ in 0..(WINDOW_BEATS + 17) {
            stream.ingest(Beat {
                ibi_ms: 1000.0,
                artifact: false,
                hr: 60.0,
            });
        }
        let snapshot = stream.snapshot("test", true);
        assert_eq!(snapshot.beats, WINDOW_BEATS + 17);
        assert_eq!(stream.ibis.as_slice().len(), WINDOW_BEATS);
    }
}
impl Drop for App {
    fn drop(&mut self) {
        let mut snap = self.shared.lock().unwrap().clone();
        let samples = self.session_samples.lock().unwrap().clone();
        persist_snapshot(&mut snap, &samples);
        *self.shared.lock().unwrap() = snap;
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let snap = self.shared.lock().unwrap().clone();
        self.finder.update(&snap);
        if self.view == View::Finder {
            self.pacer_rate = FINDER_RATES[self.finder.rate_index];
            self.pacer_enabled = self.finder.active && !self.finder.resting;
        }

        egui::CentralPanel::default()
            .frame(

                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(20, 16)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new("Resonance")
                                        .size(27.0)
                                        .strong()
                                        .color(TEXT),
                                );
                                ui.label(
                                    egui::RichText::new(
                                        "Slow breathing · real-time HRV biofeedback",
                                    )
                                    .size(13.0)
                                    .color(MUTED),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    pill(
                                        ui,
                                        if snap.connected { "LIVE" } else { "OFFLINE" },
                                        if snap.connected { ACCENT } else { RED },
                                    );
                                    ui.label(
                                        egui::RichText::new(duration(snap.elapsed))
                                            .size(13.0)
                                            .color(MUTED),
                                    );
                                },
                            );
                        });
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui
                                .selectable_label(self.view == View::Train, "TRAIN")
                                .clicked()
                            {
                                self.view = View::Train;
                            }
                            if ui
                                .selectable_label(self.view == View::Sessions, "SESSIONS")
                                .clicked()
                            {
                                self.view = View::Sessions;
                            }
                            if ui
                                .selectable_label(self.view == View::Finder, "FIND RATE")
                                .clicked()
                            {
                                self.view = View::Finder;
                            }
                        });

                        if self.view == View::Sessions {
                            session_history(ui, &snap.sessions, &mut self.selected_session);
                        } else if self.view == View::Finder {
                            resonance_finder(ui, &snap, self);
                        } else {
                            mobile_dashboard(ui, &snap, self);
                        }
                    });
            });

        ui.ctx().request_repaint_after(Duration::from_millis(if self.finder.active
            || (snap.connected && self.pacer_enabled)
        {
            50
        } else {
            250
        }));
    }
}

fn spawn_reader(
    shared: &Arc<Mutex<Snapshot>>,
    session_samples: &Arc<Mutex<Vec<archive::BeatRecord>>>,
    platform: emwave::AndroidContext,
) {
    let reader_shared = shared.clone();
    let reader_samples = session_samples.clone();
    std::thread::spawn(move || reader_loop(reader_shared, reader_samples, platform));
}

fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 800.0])
            .with_min_inner_size([860.0, 640.0]),
        ..Default::default()
    }
}

#[cfg(not(target_os = "android"))]
pub fn desktop_main() -> eframe::Result {
    let shared = Arc::new(Mutex::new(Snapshot::default()));
    let session_samples = Arc::new(Mutex::new(Vec::new()));
    spawn_reader(&shared, &session_samples, emwave::AndroidContext);
    eframe::run_native(
        "Resonance",
        native_options(),
        Box::new(move |cc| Ok(Box::new(App::new(cc, shared, session_samples)))),
    )
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: winit::platform::android::activity::AndroidApp) {
    let platform = emwave::AndroidContext {
        vm: app.vm_as_ptr() as usize,
        activity: app.activity_as_ptr() as usize,
    };
    let _ = emwave::set_keep_screen_on(platform, true);
    let shared = Arc::new(Mutex::new(Snapshot::default()));
    let session_samples = Arc::new(Mutex::new(Vec::new()));
    spawn_reader(&shared, &session_samples, platform);
    let mut options = native_options();
    options.android_app = Some(app);
    let _ = eframe::run_native(
        "Resonance",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, shared, session_samples)))),
    );
    let _ = emwave::set_keep_screen_on(platform, false);
}
 
