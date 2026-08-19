#![cfg_attr(target_os = "android", no_main)]
//! emWave2 resonance trainer: 6 bpm breath pacer + live HR/HRV/resonance.
//!
//! Reader thread talks HID and publishes a snapshot; the egui thread renders.

mod emwave;
mod metrics;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;
use parking_lot::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui;
use metrics::{HrvMetrics, Resonance};

const WINDOW_BEATS: usize = 240;
const CYCLE_SEC: f64 = 10.0; // 6 bpm
const INHALE_SEC: f64 = 4.0;

#[derive(Clone, Default)]
struct Snapshot {
    connected: bool,
    status: String,
    hr: f64,
    ibi: f64,
    beats: usize,
    artifacts: usize,
    elapsed: f64,
    hrv: Option<HrvMetrics>,
    res: Option<Resonance>,
    series: Vec<f64>,
}

// --------------------------------------------------------------------------
// Stream parsing & accumulation
// --------------------------------------------------------------------------

struct Beat {
    ibi_ms: f64,
    artifact: bool,
    hr: f64,
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Parse one `<2 I=NNN R=F H=NN >` record line.
fn parse_ibi(line: &[u8]) -> Option<Beat> {
    let i = find(line, b"<2 I=")? + 5;
    let mut j = i;
    let mut ibi = 0.0;
    while j < line.len() && line[j].is_ascii_digit() {
        ibi = ibi * 10.0 + (line[j] - b'0') as f64;
        j += 1;
    }
    if ibi <= 0.0 {
        return None;
    }
    let artifact = find(line, b"R=")
        .and_then(|r| line.get(r + 2))
        .map(|&b| b == b'T')
        .unwrap_or(false);
    let mut hr = 0.0;
    if let Some(h) = find(line, b"H=") {
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
    ibis: Vec<(f64, f64)>, // (beat_time_s, ibi_ms) artifact-free
    raw: Vec<f64>,
    last_hr: f64,
    last_ibi: f64,
    artifacts: usize,
    beat_time: f64,
    started: Instant,
}

impl HeartStream {
    fn new() -> Self {
        HeartStream {
            ibis: Vec::new(),
            raw: Vec::new(),
            last_hr: 0.0,
            last_ibi: 0.0,
            artifacts: 0,
            beat_time: 0.0,
            started: Instant::now(),
        }
    }

    fn ingest(&mut self, b: Beat) {
        self.last_ibi = b.ibi_ms;
        if b.hr > 0.0 {
            self.last_hr = b.hr;
        }
        self.beat_time += b.ibi_ms / 1000.0;
        if b.artifact {
            self.artifacts += 1;
        } else {
            self.ibis.push((self.beat_time, b.ibi_ms));
            if self.ibis.len() > WINDOW_BEATS {
                self.ibis.drain(..self.ibis.len() - WINDOW_BEATS);
            }
        }
        self.raw.push(b.ibi_ms);
        if self.raw.len() > WINDOW_BEATS {
            self.raw.drain(..self.raw.len() - WINDOW_BEATS);
        }
    }

    fn snapshot(&self, status: &str, connected: bool) -> Snapshot {
        Snapshot {
            connected,
            status: status.to_string(),
            hr: self.last_hr,
            ibi: self.last_ibi,
            beats: self.ibis.len(),
            artifacts: self.artifacts,
            elapsed: self.started.elapsed().as_secs_f64(),
            hrv: metrics::hrv_metrics(&self.ibis),
            res: metrics::resonance(&self.ibis),
            series: self.raw.clone(),
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
fn reader_loop(shared: Arc<Mutex<Snapshot>>, platform: emwave::AndroidContext) {
    diagnostic("reader thread started");
    loop {
        diagnostic("opening emWave2");
        match emwave::Device::open_and_start(platform) {
            Ok(mut dev) => {
                diagnostic("emWave2 opened and session started");
                let mut stream = HeartStream::new();
                let mut parser = BeatParser::new();
                *shared.lock() = stream.snapshot("connected", true);
                loop {
                    match dev.read_report(150) {
                        Ok(Some(rep)) => {
                            if rep[0] == 0x75 {
                                parser.feed(&rep[4..4 + rep[3] as usize]);
                                while let Some(b) = parser.next() {
                                    stream.ingest(b);
                                }
                                *shared.lock() = stream.snapshot("connected", true);
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            diagnostic(format!("HID read error: {e}"));
                            *shared.lock() = stream.snapshot(&format!("read error: {e}"), false);
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                diagnostic(format!("USB/HID open error: {e}"));
                let mut snap = Snapshot::default();
                snap.status = e;
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
    started: Instant,
    pacer_enabled: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, shared: Arc<Mutex<Snapshot>>) -> Self {
        configure_style(&cc.egui_ctx);
        Self {
            shared,
            started: Instant::now(),
            pacer_enabled: true,
        }
    }

    fn pacer(&self) -> (&'static str, f64, f64) {
        let pos = self.started.elapsed().as_secs_f64() % CYCLE_SEC;
        if pos < INHALE_SEC {
            ("INHALE", pos / INHALE_SEC, pos / INHALE_SEC)
        } else {
            let progress = (pos - INHALE_SEC) / (CYCLE_SEC - INHALE_SEC);
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

fn metric_card(
    ui: &mut egui::Ui,
    label: &str,
    value: &str,
    unit: &str,
    accent: egui::Color32,
) {
    card().show(ui, |ui| {
        ui.set_min_height(78.0);
        section_label(ui, label);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(value)
                    .size(34.0)
                    .strong()
                    .color(TEXT),
            );
            ui.label(egui::RichText::new(unit).size(13.0).color(MUTED));
        });
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 3.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 2.0, accent);
    });
}

fn small_stat(ui: &mut egui::Ui, label: &str, value: &str, caption: &str) {
    egui::Frame::new()
        .fill(CARD_ALT)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(13)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_min_height(58.0);
            section_label(ui, label);
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(value)
                    .size(19.0)
                    .strong()
                    .color(TEXT),
            );
            ui.label(egui::RichText::new(caption).size(11.0).color(MUTED));
        });
}

fn duration(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    format!("{:02}:{:02}", total / 60, total % 60)
}
fn mobile_dashboard(ui: &mut egui::Ui, snap: &Snapshot, app: &mut App) {
    let state = snap.res.map(|r| r.state).unwrap_or("BUILDING");
    let state_tint = state_color(state);

    card().show(ui, |ui| {
        ui.horizontal(|ui| {
            section_label(ui, "BREATH PACER");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.checkbox(&mut app.pacer_enabled, "Enabled");
            });
        });
        ui.label(
            egui::RichText::new(if app.pacer_enabled {
                "Follow the circle gently"
            } else {
                "Pacer paused · measurements continue"
            })
            .size(13.0)
            .color(MUTED),
        );
        let (phase, progress, amount) = if app.pacer_enabled {
            app.pacer()
        } else {
            ("PAUSED", 0.0, 0.0)
        };
        let tint = if !app.pacer_enabled {
            MUTED
        } else if phase == "INHALE" {
            ACCENT
        } else {
            BLUE
        };
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 150.0), egui::Sense::hover());
        let center = rect.center();
        let painter = ui.painter();
        painter.circle_filled(center, 68.0, egui::Color32::from_rgb(12, 19, 26));
        painter.circle_stroke(center, 68.0, egui::Stroke::new(1.0, BORDER));
        let radius = 30.0 + 32.0 * amount as f32;
        painter.circle_filled(
            center,
            radius,
            egui::Color32::from_rgba_premultiplied(tint.r(), tint.g(), tint.b(), 52),
        );
        painter.circle_stroke(center, radius, egui::Stroke::new(3.0, tint));
        painter.text(
            center - egui::vec2(0.0, 6.0),
            egui::Align2::CENTER_CENTER,
            phase,
            egui::FontId::proportional(18.0),
            TEXT,
        );
        painter.text(
            center + egui::vec2(0.0, 16.0),
            egui::Align2::CENTER_CENTER,
            "6.0 breaths / min",
            egui::FontId::proportional(11.0),
            TEXT,
        );
        ui.add(
            egui::ProgressBar::new(progress as f32)
                .desired_width(ui.available_width())
                .corner_radius(8)
                .fill(tint)
                .text(if app.pacer_enabled {
                    "4 sec in  ·  6 sec out"
                } else {
                    "Pacer disabled"
                }),
        );
    });

    ui.add_space(10.0);

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
    ui.columns(2, |cols| {
        metric_card(&mut cols[0], "HEART RATE", &hr, "bpm", RED);
        metric_card(&mut cols[1], "INTER-BEAT", &ibi, "ms", BLUE);
    });

    ui.add_space(10.0);
    card().show(ui, |ui| {
        ui.horizontal(|ui| {
            section_label(ui, "RESONANCE ALIGNMENT");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                pill(ui, state, state_tint);
            });
        });
        let score = snap.res.map(|r| r.score).unwrap_or(0.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(if snap.res.is_some() {
                    format!("{score:.0}")
                } else {
                    "—".to_owned()
                })
                .size(38.0)
                .strong()
                .color(TEXT),
            );
            ui.label(egui::RichText::new("%").size(16.0).color(MUTED));
        });
        ui.add(
            egui::ProgressBar::new((score / 100.0) as f32)
                .desired_width(ui.available_width())
                .corner_radius(8)
                .fill(state_tint)
                .text(""),
        );
        let detail = snap
            .res
            .map(|r| format!("peak {:.1} bpm  ·  LF {:.0}%  ·  HF {:.0}%", r.bpm, r.lf_nu, r.hf_nu))
            .unwrap_or_else(|| "collecting clean beats…".to_owned());
        ui.label(egui::RichText::new(detail).size(12.0).color(MUTED));
    });

    ui.add_space(10.0);
    card().show(ui, |ui| {
        ui.horizontal(|ui| {
            section_label(ui, "HEART RHYTHM");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("{} beats", snap.beats))
                        .size(11.0)
                        .color(MUTED),
                );
            });
        });
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 145.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 11.0, CHART_BG);
        let series = &snap.series;
        if series.len() >= 2 {
            let lo = series.iter().cloned().fold(f64::INFINITY, f64::min) - 35.0;
            let hi = series.iter().cloned().fold(f64::NEG_INFINITY, f64::max) + 35.0;
            let range = (hi - lo).max(100.0);
            let points: Vec<egui::Pos2> = series
                .iter()
                .enumerate()
                .map(|(i, &v)| {
                    let x = rect.left() + 8.0
                        + (i as f32 / (series.len() - 1) as f32) * (rect.width() - 16.0);
                    let y = rect.bottom() - 8.0
                        - (((v - lo) / range) as f32) * (rect.height() - 16.0);
                    egui::pos2(x, y)
                })
                .collect();
            painter.add(egui::Shape::line(points, egui::Stroke::new(2.0, ACCENT)));
        } else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Waiting for clean beats…",
                egui::FontId::proportional(13.0),
                MUTED,
            );
        }
    });

    ui.add_space(10.0);
    ui.columns(2, |cols| {
        let rmssd = snap.hrv.map(|h| format!("{:.1}", h.rmssd)).unwrap_or_else(|| "—".to_owned());
        let sdnn = snap.hrv.map(|h| format!("{:.1}", h.sdnn)).unwrap_or_else(|| "—".to_owned());
        // Second row values are intentionally scoped here.
        small_stat(&mut cols[0], "RMSSD", &rmssd, "ms · short-term HRV");
        small_stat(&mut cols[1], "SDNN", &sdnn, "ms · total variability");
    });
    ui.add_space(8.0);
    ui.columns(2, |cols| {
        let pnn50 = snap.hrv.map(|h| format!("{:.1}", h.pnn50)).unwrap_or_else(|| "—".to_owned());
        let peak = snap.res.map(|r| format!("{:.1}", r.bpm)).unwrap_or_else(|| "—".to_owned());
        small_stat(&mut cols[0], "pNN50", &pnn50, "% · successive beats");
        small_stat(&mut cols[1], "PEAK RATE", &peak, "bpm · dominant rhythm");
    });

    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(format!("{} artifacts  ·  {}", snap.artifacts, snap.status))
            .size(11.0)
            .color(MUTED),
    );
}


impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let snap = self.shared.lock().clone();
        let state = snap.res.map(|r| r.state).unwrap_or("BUILDING");
        let state_tint = state_color(state);

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

                        let mobile = ui.available_width() < 760.0;
                        if mobile {
                            mobile_dashboard(ui, &snap, self);
                        } else {
                        ui.add_space(14.0);

                        ui.columns(2, |cols| {
                            card().show(&mut cols[0], |ui| {
                                ui.set_min_height(210.0);
                                ui.horizontal(|ui| {
                                    section_label(ui, "BREATH PACER");
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .checkbox(&mut self.pacer_enabled, "Enabled")
                                                .changed()
                                                && self.pacer_enabled
                                            {
                                                self.started = Instant::now();
                                            }
                                        },
                                    );
                                });
                                ui.label(
                                    egui::RichText::new(if self.pacer_enabled {
                                        "Follow the circle gently"
                                    } else {
                                        "Pacer paused · measurements continue"
                                    })
                                    .size(14.0)
                                    .color(MUTED),
                                );

                                let (phase, progress, breath_amount) = if self.pacer_enabled {
                                    self.pacer()
                                } else {
                                    ("PAUSED", 0.0, 0.0)
                                };
                                let phase_tint = if !self.pacer_enabled {
                                    MUTED
                                } else if phase == "INHALE" {
                                    ACCENT
                                } else {
                                    BLUE
                                };
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), 150.0),
                                    egui::Sense::hover(),
                                );
                                let center = rect.center();
                                let painter = ui.painter();
                                painter.circle_filled(
                                    center,
                                    72.0,
                                    egui::Color32::from_rgb(12, 19, 26),
                                );
                                painter.circle_stroke(
                                    center,
                                    72.0,
                                    egui::Stroke::new(1.0, BORDER),
                                );
                                let radius = 32.0 + 34.0 * breath_amount as f32;
                                painter.circle_filled(
                                    center,
                                    radius,
                                    egui::Color32::from_rgba_premultiplied(
                                        phase_tint.r(),
                                        phase_tint.g(),
                                        phase_tint.b(),
                                        52,
                                    ),
                                );
                                painter.circle_stroke(
                                    center,
                                    radius,
                                    egui::Stroke::new(3.0, phase_tint),
                                );
                                painter.text(
                                    center - egui::vec2(0.0, 7.0),
                                    egui::Align2::CENTER_CENTER,
                                    phase,
                                    egui::FontId::proportional(19.0),
                                    TEXT,
                                );
                                painter.text(
                                    center + egui::vec2(0.0, 17.0),
                                    egui::Align2::CENTER_CENTER,
                                    if self.pacer_enabled {
                                        "6.0 breaths / min"
                                    } else {
                                        "live metrics remain active"
                                    },
                                    egui::FontId::proportional(12.0),
                                    TEXT,
                                );
                                ui.add(
                                    egui::ProgressBar::new(progress as f32)
                                        .desired_width(ui.available_width())
                                        .corner_radius(8)
                                        .fill(phase_tint)
                                        .text(if self.pacer_enabled {
                                            "4 sec in  ·  6 sec out"
                                        } else {
                                            "Pacer disabled"
                                        }),
                                );
                            });

                            cols[1].columns(2, |metrics| {
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
                                metric_card(&mut metrics[0], "HEART RATE", &hr, "bpm", RED);
                                metric_card(&mut metrics[1], "INTER-BEAT", &ibi, "ms", BLUE);
                            });
                            cols[1].add_space(10.0);
                            card().show(&mut cols[1], |ui| {
                                ui.set_min_height(112.0);
                                ui.horizontal(|ui| {
                                    section_label(ui, "RESONANCE ALIGNMENT");
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| pill(ui, state, state_tint),
                                    );
                                });
                                let score = snap.res.map(|r| r.score).unwrap_or(0.0);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(if snap.res.is_some() {
                                            format!("{score:.0}")
                                        } else {
                                            "—".to_owned()
                                        })
                                        .size(40.0)
                                        .strong()
                                        .color(TEXT),
                                    );
                                    ui.label(
                                        egui::RichText::new("%")
                                            .size(17.0)
                                            .color(MUTED),
                                    );
                                });
                                ui.add(
                                    egui::ProgressBar::new((score / 100.0) as f32)
                                        .desired_width(ui.available_width())
                                        .corner_radius(8)
                                        .fill(state_tint)
                                        .text(""),
                                );
                                let detail = snap
                                    .res
                                    .map(|r| {
                                        format!(
                                            "peak {:.3} Hz  ·  LF {:.0}%  ·  HF {:.0}%  ·  LF/HF {:.1}",
                                            r.peak_freq, r.lf_nu, r.hf_nu, r.lf_hf
                                        )
                                    })
                                    .unwrap_or_else(|| "collecting clean beats…".to_owned());
                                ui.label(
                                    egui::RichText::new(detail)
                                        .size(12.0)
                                        .color(MUTED),
                                );
                            });
                        });

                        ui.add_space(12.0);

                        card().show(ui, |ui| {
                            ui.horizontal(|ui| {
                                section_label(ui, "HEART RHYTHM");
                                let chart_detail = snap
                                    .hrv
                                    .map(|h| {
                                        format!(
                                            "{} beats  ·  {} artifacts  ·  RSA span {:.0} ms",
                                            snap.beats, snap.artifacts, h.hr_max_min
                                        )
                                    })
                                    .unwrap_or_else(|| {
                                        format!(
                                            "{} beats  ·  {} artifacts",
                                            snap.beats, snap.artifacts
                                        )
                                    });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            egui::RichText::new(chart_detail)
                                                .size(11.0)
                                                .color(MUTED),
                                        );
                                    },
                                );
                            });
                            let (grect, _) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 134.0),
                                egui::Sense::hover(),
                            );
                            let painter = ui.painter_at(grect);
                            painter.rect_filled(grect, 11.0, CHART_BG);
                            painter.rect_stroke(
                                grect,
                                11.0,
                                egui::Stroke::new(1.0, BORDER),
                                egui::StrokeKind::Inside,
                            );
                            for i in 1..4 {
                                let y = egui::lerp(grect.top()..=grect.bottom(), i as f32 / 4.0);
                                painter.line_segment(
                                    [
                                        egui::pos2(grect.left() + 8.0, y),
                                        egui::pos2(grect.right() - 8.0, y),
                                    ],
                                    egui::Stroke::new(
                                        1.0,
                                        egui::Color32::from_rgb(27, 38, 49),
                                    ),
                                );
                            }

                            let series = &snap.series;
                            if series.len() >= 2 {
                                let lo =
                                    series.iter().cloned().fold(f64::INFINITY, f64::min) - 35.0;
                                let hi = series
                                    .iter()
                                    .cloned()
                                    .fold(f64::NEG_INFINITY, f64::max)
                                    + 35.0;
                                let range = (hi - lo).max(100.0);
                                let points: Vec<egui::Pos2> = series
                                    .iter()
                                    .enumerate()
                                    .map(|(i, &v)| {
                                        let x = grect.left()
                                            + 10.0
                                            + (i as f32 / (series.len() - 1) as f32)
                                                * (grect.width() - 20.0);
                                        let y = grect.bottom()
                                            - 10.0
                                            - (((v - lo) / range) as f32)
                                                * (grect.height() - 20.0);
                                        egui::pos2(x, y)
                                    })
                                    .collect();
                                painter.add(egui::Shape::line(
                                    points.clone(),
                                    egui::Stroke::new(2.2, ACCENT),
                                ));
                                if let Some(last) = points.last() {
                                    painter.circle_filled(*last, 4.5, ACCENT);
                                    painter.circle_stroke(
                                        *last,
                                        7.5,
                                        egui::Stroke::new(
                                            1.0,
                                            egui::Color32::from_rgba_premultiplied(
                                                ACCENT.r(),
                                                ACCENT.g(),
                                                ACCENT.b(),
                                                90,
                                            ),
                                        ),
                                    );
                                }
                                painter.text(
                                    grect.left_top() + egui::vec2(12.0, 10.0),
                                    egui::Align2::LEFT_TOP,
                                    format!("{hi:.0}"),
                                    egui::FontId::monospace(10.0),
                                    MUTED,
                                );
                                painter.text(
                                    grect.left_bottom() + egui::vec2(12.0, -10.0),
                                    egui::Align2::LEFT_BOTTOM,
                                    format!("{lo:.0}"),
                                    egui::FontId::monospace(10.0),
                                    MUTED,
                                );
                            } else {
                                painter.text(
                                    grect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "Waiting for clean beats…",
                                    egui::FontId::proportional(14.0),
                                    MUTED,
                                );
                            }
                        });

                        ui.add_space(10.0);

                        ui.columns(4, |stats| {
                            let rmssd = snap
                                .hrv
                                .map(|h| format!("{:.1}", h.rmssd))
                                .unwrap_or_else(|| "—".to_owned());
                            let sdnn = snap
                                .hrv
                                .map(|h| format!("{:.1}", h.sdnn))
                                .unwrap_or_else(|| "—".to_owned());
                            let pnn50 = snap
                                .hrv
                                .map(|h| format!("{:.1}", h.pnn50))
                                .unwrap_or_else(|| "—".to_owned());
                            let peak = snap
                                .res
                                .map(|r| format!("{:.1}", r.bpm))
                                .unwrap_or_else(|| "—".to_owned());
                            small_stat(&mut stats[0], "RMSSD", &rmssd, "ms · short-term HRV");
                            small_stat(&mut stats[1], "SDNN", &sdnn, "ms · total variability");
                            small_stat(&mut stats[2], "pNN50", &pnn50, "% · successive beats");
                            small_stat(&mut stats[3], "PEAK RATE", &peak, "bpm · dominant rhythm");
                        });
                        }
                    });
            });

        ui.ctx().request_repaint_after(Duration::from_millis(if self.pacer_enabled {
            50
        } else {
            250
        }));
    }
}

fn spawn_reader(shared: &Arc<Mutex<Snapshot>>, platform: emwave::AndroidContext) {
    let reader_shared = shared.clone();
    std::thread::spawn(move || reader_loop(reader_shared, platform));
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
    spawn_reader(&shared, emwave::AndroidContext);
    eframe::run_native(
        "Resonance",
        native_options(),
        Box::new(move |cc| Ok(Box::new(App::new(cc, shared)))),
    )
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: winit::platform::android::activity::AndroidApp) {
    let platform = emwave::AndroidContext {
        vm: app.vm_as_ptr() as usize,
        activity: app.activity_as_ptr() as usize,
    };
    let shared = Arc::new(Mutex::new(Snapshot::default()));
    spawn_reader(&shared, platform);
    let mut options = native_options();
    options.android_app = Some(app);
    let _ = eframe::run_native(
        "Resonance",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, shared)))),
    );
}
 
