use super::*;

pub(crate) fn mobile_dashboard(ui: &mut egui::Ui, snap: &Snapshot, app: &mut App) {
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
            section_label(ui, "BREATHING GUIDE");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(snap.connected, |ui| {
                    ui.checkbox(&mut app.pacer_enabled, "Pacer on");
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

    card().inner_margin(egui::Margin::same(12)).show(ui, |ui| {
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
                    .map(|r| format!("Dominant rhythm: {:.1} bpm", r.bpm))
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
            compact_stat(&mut cols[1], "BEAT INTERVAL", &ibi, "ms");
        });
    });

    ui.add_space(6.0);
    card().inner_margin(egui::Margin::same(10)).show(ui, |ui| {
        ui.horizontal(|ui| {
            section_label(ui, "HEART RHYTHM");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "{} beats · {} artifacts",
                        snap.beats, snap.artifacts
                    ))
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
            let hi = snap
                .series
                .iter()
                .cloned()
                .fold(f64::NEG_INFINITY, f64::max)
                + 25.0;
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
        egui::CollapsingHeader::new("HRV details")
            .default_open(false)
            .show(ui, |ui| {
                if let Some(resonance) = snap.res {
                    ui.label(
                        egui::RichText::new(format!(
                            "LF {:.0}% · HF {:.0}%",
                            resonance.lf_nu, resonance.hf_nu
                        ))
                        .size(10.0)
                        .color(MUTED),
                    );
                }
                ui.columns(4, |cols| {
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
                    compact_stat(&mut cols[0], "RMSSD", &rmssd, "ms");
                    compact_stat(&mut cols[1], "SDNN", &sdnn, "ms");
                    compact_stat(&mut cols[2], "pNN50", &pnn50, "%");
                    compact_stat(&mut cols[3], "PEAK", &peak, "bpm");
                });
            });
    });
}
