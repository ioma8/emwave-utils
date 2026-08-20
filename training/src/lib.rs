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
mod ui_finder;
mod ui_sessions;
mod ui_train;
use parking_lot::Mutex;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use analysis::NnSeries;
use eframe::egui;
use metrics::{HrvMetrics, Resonance};
use runtime::{FinderRuntime, FINDER_RATES, MIN_TRIAL_DATA_SECONDS, REST_SECONDS, TRIAL_SECONDS};
const WINDOW_BEATS: usize = 240;
const GRAPH_BEATS: usize = 120;
const DEFAULT_PACER_RATE: f64 = 6.0;
const INHALE_FRACTION: f64 = 0.4;

#[derive(Clone, Default)]
struct Snapshot {
    connected: bool,
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
    sessions: Arc<Vec<archive::SessionRecord>>,
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
        .position(|window| window == b"<2 I=")?
        + 5;
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
    if let Some(h) = line.windows(2).position(|window| window == b"H=") {
        let mut k = h + 2;
        while k < line.len() && line[k].is_ascii_digit() {
            hr = hr * 10.0 + (line[k] - b'0') as f64;
            k += 1;
        }
    }
    Some(Beat {
        ibi_ms: ibi,
        artifact,
        hr,
    })
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

fn report_payload(report: &[u8], report_id: u8) -> Option<&[u8]> {
    if report.len() < 4 || report[0] != report_id {
        return None;
    }
    let length = report[3] as usize;
    report.get(4..4 + length)
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
    sessions: Arc<Vec<archive::SessionRecord>>,
}

impl HeartStream {
    fn new(sessions: Arc<Vec<archive::SessionRecord>>) -> Self {
        HeartStream {
            ibis: NnSeries::default(),
            analysis_received: NnSeries::default(),
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

    fn snapshot(&self, connected: bool) -> Snapshot {
        Snapshot {
            connected,
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
    let record = archive::SessionRecord {
        started_unix: snap.session_started_unix,
        duration_secs: snap.elapsed,
        mean_hr: snap.mean_hr,
        mean_score: snap.mean_score,
        beats: snap.beats,
        artifacts: snap.artifacts,
        samples: samples.to_vec(),
    };
    if let Err(error) = archive::append(Arc::make_mut(&mut snap.sessions), record) {
        diagnostic(format!("session archive error: {error}"));
    }
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
                let sessions = match archive::load() {
                    Ok(sessions) => sessions,
                    Err(error) => {
                        diagnostic(format!("session archive error: {error}"));
                        Vec::new()
                    }
                };
                let mut stream = HeartStream::new(Arc::new(sessions));
                let reader_started = Instant::now();
                session_samples.lock().clear();
                let mut parser = BeatParser::new();
                *shared.lock() = stream.snapshot(true);
                loop {
                    match dev.read_report(150) {
                        Ok(Some(rep)) => {
                            if let Some(payload) = report_payload(&rep, 0x75) {
                                parser.feed(payload);
                                while let Some(b) = parser.next() {
                                    let received_secs = reader_started.elapsed().as_secs_f64();
                                    let sample = (b.ibi_ms, b.artifact, b.hr);
                                    stream.ingest(b);
                                    if !sample.1 {
                                        stream.analysis_received.push(received_secs, sample.0);
                                        stream.analysis_received.trim_to(WINDOW_BEATS);
                                    }
                                    session_samples.lock().push(archive::BeatRecord {
                                        time_secs: stream.beat_time,
                                        received_secs,
                                        ibi_ms: sample.0,
                                        artifact: sample.1,
                                        hr: sample.2,
                                    });
                                }
                                *shared.lock() = stream.snapshot(true);
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            diagnostic(format!("HID read error: {e}"));
                            let mut snap = stream.snapshot(false);
                            let samples = session_samples.lock().clone();
                            persist_snapshot(&mut snap, &samples);
                            *shared.lock() = snap;
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                diagnostic(format!("USB/HID open error: {e}"));
                let mut snap = shared.lock().clone();
                snap.connected = false;
                *shared.lock() = snap;
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
        if let Err(error) = archive::load().map(|sessions| {
            shared.lock().sessions = Arc::new(sessions);
        }) {
            diagnostic(format!("session archive error: {error}"));
        }
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
        Self::pacer_at(self.pacer_rate, self.started.elapsed().as_secs_f64())
    }

    fn pacer_at(rate: f64, elapsed: f64) -> (&'static str, f64, f64) {
        let cycle = 60.0 / rate.max(1.0);
        let inhale = cycle * INHALE_FRACTION;
        let pos = elapsed % cycle;
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
    style
        .text_styles
        .insert(egui::TextStyle::Heading, egui::FontId::proportional(27.0));
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
    ui.label(egui::RichText::new(text).size(11.0).strong().color(MUTED));
}

fn duration(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    if total >= 3_600 {
        format!(
            "{:02}:{:02}:{:02}",
            total / 3_600,
            total / 60 % 60,
            total % 60
        )
    } else {
        format!("{:02}:{:02}", total / 60, total % 60)
    }
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
impl Drop for App {
    fn drop(&mut self) {
        let mut snap = self.shared.lock().clone();
        let samples = self.session_samples.lock().clone();
        persist_snapshot(&mut snap, &samples);
        *self.shared.lock() = snap;
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        #[cfg(target_os = "android")]
        if ui.input(|input| {
            input.key_pressed(egui::Key::BrowserBack) || input.key_pressed(egui::Key::Escape)
        }) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let snap = self.shared.lock().clone();
        self.finder.update(&snap);

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(20, 16)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Resonance")
                            .size(27.0)
                            .strong()
                            .color(TEXT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        pill(
                            ui,
                            if snap.connected { "LIVE" } else { "OFFLINE" },
                            if snap.connected { ACCENT } else { MUTED },
                        );
                        if snap.connected {
                            ui.label(
                                egui::RichText::new(duration(snap.elapsed))
                                    .size(13.0)
                                    .color(MUTED),
                            );
                        }
                    });
                });
                ui.add_space(8.0);
                ui.columns(3, |cols| {
                    for (column, view, label) in [
                        (0, View::Train, "Train"),
                        (1, View::Finder, "Find rate"),
                        (2, View::Sessions, "Sessions"),
                    ] {
                        let width = cols[column].available_width();
                        if cols[column]
                            .add_sized(
                                [width, 34.0],
                                egui::Button::selectable(self.view == view, label),
                            )
                            .clicked()
                        {
                            self.view = view;
                        }
                    }
                });
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if self.view == View::Sessions {
                            ui_sessions::session_history(
                                ui,
                                snap.sessions.as_slice(),
                                &mut self.selected_session,
                            );
                        } else if self.view == View::Finder {
                            ui_finder::resonance_finder(ui, &snap, self);
                        } else {
                            ui_train::mobile_dashboard(ui, &snap, self);
                        }
                    });
            });

        ui.ctx().request_repaint_after(Duration::from_millis(
            if self.finder.active() || (snap.connected && self.pacer_enabled) {
                50
            } else {
                250
            },
        ));
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
#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App {
            shared: Arc::new(Mutex::new(Snapshot::default())),
            session_samples: Arc::new(Mutex::new(Vec::new())),
            started: Instant::now(),
            pacer_enabled: true,
            pacer_rate: DEFAULT_PACER_RATE,
            view: View::Finder,
            finder: FinderRuntime::default(),
            selected_session: None,
        }
    }

    #[test]
    fn sequential_finder_runs_every_rate_then_stops() {
        let mut app = app();
        let snap = Snapshot::default();
        app.finder.start_trial(0, true, &snap);
        for expected in 1..FINDER_RATES.len() {
            app.finder.finish_trial(&snap, MIN_TRIAL_DATA_SECONDS);
            assert!(app.finder.active());
            assert_eq!(app.finder.rate_index, expected);
        }
        app.finder.finish_trial(&snap, MIN_TRIAL_DATA_SECONDS);
        assert!(!app.finder.active());
    }

    #[test]
    fn sequential_finder_inserts_natural_breathing_rest() {
        let mut app = app();
        let snap = Snapshot {
            connected: true,
            ..Default::default()
        };
        app.finder.start_trial(0, true, &snap);
        app.finder.finish_trial(&snap, MIN_TRIAL_DATA_SECONDS);
        assert!(app.finder.active());
        assert!(app.finder.resting());
        assert_eq!(app.finder.rate_index, 1);
        app.finder.complete_rest_for_test();
        app.finder.update(&snap);
        assert!(!app.finder.resting());
        assert_eq!(app.finder.rate_index, 1);
        assert!(app.finder.active());
    }

    #[test]
    fn disconnect_interrupts_trial_without_scoring() {
        let mut app = app();
        let connected = Snapshot::default();
        app.finder.start_trial(0, true, &connected);
        let disconnected = Snapshot {
            connected: false,
            ..connected
        };
        app.finder.update(&disconnected);
        assert!(!app.finder.active());
        assert!(app.finder.interrupted);
        assert!(app.finder.results.is_empty());
    }

    #[test]
    fn finder_uses_receipt_time_for_trial_boundaries() {
        let mut app = app();
        let snap = Snapshot {
            analysis_received: NnSeries::from_vec(vec![(12.5, 800.0)]),
            ..Default::default()
        };
        app.finder.start_trial(0, false, &snap);
        assert_eq!(app.finder.data_started_s(), Some(12.5));
    }

    #[test]
    fn artifact_heart_rate_does_not_change_session_mean() {
        let mut stream = HeartStream::new(Arc::new(Vec::new()));
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
        assert_eq!(stream.snapshot(true).mean_hr, 60.0);
    }

    #[test]
    fn duration_includes_hours_for_long_sessions() {
        assert_eq!(duration(3_661.0), "01:01:01");
    }

    #[test]
    fn archive_beat_count_exceeds_analysis_window() {
        let mut stream = HeartStream::new(Arc::new(Vec::new()));
        for _ in 0..(WINDOW_BEATS + 17) {
            stream.ingest(Beat {
                ibi_ms: 1000.0,
                artifact: false,
                hr: 60.0,
            });
        }
        let snapshot = stream.snapshot(true);
        assert_eq!(snapshot.beats, WINDOW_BEATS + 17);
        assert_eq!(stream.ibis.as_slice().len(), WINDOW_BEATS);
    }
}
