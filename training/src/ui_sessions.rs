use super::*;

fn session_graph(ui: &mut egui::Ui, session: &archive::SessionRecord) {
    card().show(ui, |ui| {
        ui.horizontal(|ui| {
            section_label(ui, "FULL SESSION HEART RHYTHM");
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
        let timeline_secs = session
            .samples
            .last()
            .map(|sample| sample.time_secs)
            .unwrap_or_default()
            .max(session.duration_secs)
            .max(1.0);
        let viewport_width = ui.available_width();
        let graph_width = (timeline_secs as f32 * 4.0)
            .max(session.samples.len() as f32 * 3.0)
            .max(viewport_width);
        if graph_width > viewport_width + 1.0 {
            ui.label(
                egui::RichText::new(format!(
                    "Full {} session continues horizontally — swipe or drag the scrollbar.",
                    duration(timeline_secs)
                ))
                .size(11.0)
                .color(MUTED),
            );
        }

        egui::ScrollArea::horizontal()
            .id_salt(("session-waveform", session.started_unix))
            .scroll_bar_visibility(
                egui::containers::scroll_area::ScrollBarVisibility::AlwaysVisible,
            )
            .show(ui, |ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(graph_width, 240.0), egui::Sense::hover());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 11.0, CHART_BG);
                let plot = egui::Rect::from_min_max(
                    rect.left_top() + egui::vec2(12.0, 12.0),
                    rect.right_bottom() - egui::vec2(12.0, 30.0),
                );
                let pixels_per_second = plot.width() / timeline_secs as f32;
                let tick_secs = [30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1_800.0, 3_600.0]
                    .into_iter()
                    .find(|seconds| *seconds as f32 * pixels_per_second >= 180.0)
                    .unwrap_or(3_600.0);
                let mut tick = 0.0;
                while tick <= timeline_secs {
                    let x = plot.left() + (tick / timeline_secs) as f32 * plot.width();
                    painter.line_segment(
                        [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
                        egui::Stroke::new(1.0, BORDER),
                    );
                    let alignment = if tick == 0.0 {
                        egui::Align2::LEFT_BOTTOM
                    } else if (timeline_secs - tick).abs() < 0.001 {
                        egui::Align2::RIGHT_BOTTOM
                    } else {
                        egui::Align2::CENTER_BOTTOM
                    };
                    painter.text(
                        egui::pos2(x, rect.bottom() - 6.0),
                        alignment,
                        duration(tick),
                        egui::FontId::monospace(10.0),
                        MUTED,
                    );
                    tick += tick_secs;
                }
                if timeline_secs - (tick - tick_secs) > 0.001 {
                    painter.line_segment(
                        [
                            egui::pos2(plot.right(), plot.top()),
                            egui::pos2(plot.right(), plot.bottom()),
                        ],
                        egui::Stroke::new(1.0, BORDER),
                    );
                    painter.text(
                        egui::pos2(plot.right(), rect.bottom() - 6.0),
                        egui::Align2::RIGHT_BOTTOM,
                        duration(timeline_secs),
                        egui::FontId::monospace(10.0),
                        MUTED,
                    );
                }

                let mut previous_clean = None;
                for sample in &session.samples {
                    let x = plot.left()
                        + (sample.time_secs.clamp(0.0, timeline_secs) / timeline_secs) as f32
                            * plot.width();
                    let y = plot.bottom() - ((sample.ibi_ms - lo) / range) as f32 * plot.height();
                    let point = egui::pos2(x, y);
                    if sample.artifact {
                        painter.circle_filled(point, 3.5, RED);
                        previous_clean = None;
                    } else {
                        if let Some(previous) = previous_clean {
                            painter.line_segment([previous, point], egui::Stroke::new(2.0, ACCENT));
                        }
                        previous_clean = Some(point);
                    }
                }
                painter.text(
                    plot.left_top() + egui::vec2(6.0, 6.0),
                    egui::Align2::LEFT_TOP,
                    format!("{hi:.0} ms"),
                    egui::FontId::monospace(10.0),
                    MUTED,
                );
                painter.text(
                    plot.left_bottom() + egui::vec2(6.0, -6.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!("{lo:.0} ms"),
                    egui::FontId::monospace(10.0),
                    MUTED,
                );
            });
        ui.label(
            egui::RichText::new("Green: clean NN intervals · red: artifact-marked intervals")
                .size(11.0)
                .color(MUTED),
        );
    });
}

fn session_detail(
    ui: &mut egui::Ui,
    session: &archive::SessionRecord,
    selected_session: &mut Option<usize>,
) {
    if ui.button("BACK TO SESSIONS").clicked() {
        *selected_session = None;
        return;
    }
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(archive::format_date(session.started_unix))
            .size(24.0)
            .strong()
            .color(TEXT),
    );
    ui.label(
        egui::RichText::new(format!(
            "{} · {} beats · {} artifacts · {:.0}% alignment · {:.0} bpm average",
            duration(session.duration_secs),
            session.beats,
            session.artifacts,
            session.mean_score,
            session.mean_hr
        ))
        .size(12.0)
        .color(MUTED),
    );
    ui.add_space(12.0);
    session_graph(ui, session);
}

pub(crate) fn session_history(
    ui: &mut egui::Ui,
    sessions: &[archive::SessionRecord],
    selected_session: &mut Option<usize>,
) {
    if let Some(index) = *selected_session {
        if let Some(session) = sessions.get(index) {
            session_detail(ui, session, selected_session);
            return;
        }
        *selected_session = None;
    }

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
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(archive::format_date(session.started_unix))
                            .size(15.0)
                            .strong()
                            .color(TEXT),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{} · {} beats · {} artifacts",
                            duration(session.duration_secs),
                            session.beats,
                            session.artifacts
                        ))
                        .size(12.0)
                        .color(MUTED),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
                                "alignment · {:.0} bpm average",
                                session.mean_hr
                            ))
                            .size(11.0)
                            .color(MUTED),
                        );
                    });
                });
            });
            if ui
                .add_sized(
                    [ui.available_width(), 32.0],
                    egui::Button::new("VIEW SESSION"),
                )
                .clicked()
            {
                *selected_session = Some(index);
            }
        });
        ui.add_space(8.0);
    }
}
