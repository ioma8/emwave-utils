use super::*;

pub(crate) fn finder_pacer(ui: &mut egui::Ui, app: &App) {
    if !app.finder.active() {
        return;
    }
    if app.finder.resting() {
        let elapsed = app.finder.rest_elapsed().min(REST_SECONDS);
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
                    .text(format!(
                        "next: {:.1} breaths/min",
                        FINDER_RATES[app.finder.rate_index]
                    )),
            );
        });
        return;
    }
    let elapsed = app.finder.trial_elapsed().min(TRIAL_SECONDS);
    let rate = FINDER_RATES[app.finder.rate_index];
    let (phase, progress, amount) = App::pacer_at(rate, elapsed);
    let tint = if phase == "INHALE" { ACCENT } else { BLUE };
    card().show(ui, |ui| {
        ui.horizontal(|ui| {
            section_label(
                ui,
                if app.finder.run_all() {
                    "RATE ASSESSMENT"
                } else {
                    "SINGLE TRIAL"
                },
            );
            pill(ui, &format!("{rate:.1} breaths/min"), tint);
        });
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 180.0),
            egui::Sense::hover(),
        );
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
                .text(format!("{:.0}s remaining", TRIAL_SECONDS - elapsed)),
        );
        if app.finder.run_all() {
            ui.label(
                egui::RichText::new(format!(
                    "Trial {} of {}",
                    app.finder.rate_index + 1,
                    FINDER_RATES.len()
                ))
                .size(11.0)
                .color(MUTED),
            );
        }
    });
}

pub(crate) fn resonance_finder(ui: &mut egui::Ui, snap: &Snapshot, app: &mut App) {
    card().inner_margin(egui::Margin::same(12)).show(ui, |ui| {
        ui.label(
            egui::RichText::new("Find your breathing rate")
                .size(20.0)
                .strong()
                .color(TEXT),
        );
        ui.label(
            egui::RichText::new(
                "Compare your heart-rhythm response at five paced breathing rates.",
            )
            .size(12.0)
            .color(MUTED),
        );
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("Five 2-minute trials with 2-minute rests · about 18 minutes")
                .size(10.0)
                .color(MUTED),
        );
    });

    ui.add_space(10.0);
    finder_pacer(ui, app);
    ui.add_space(10.0);
    if !app.finder.active() {
        card().show(ui, |ui| {
            section_label(ui, "START ASSESSMENT");
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
                    egui::Button::new("START 5-RATE ASSESSMENT"),
                )
                .clicked()
            {
                app.finder.results.clear();
                app.finder.start_trial(0, true, snap);
            }
            ui.label(
                egui::RichText::new(if snap.connected {
                    "Tests 6.5, 6.0, 5.5, 5.0, and 4.5 breaths/min."
                } else {
                    "Connect emWave2 to start."
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
                egui::RichText::new("Each single trial lasts 2 minutes.")
                    .size(12.0)
                    .color(MUTED),
            );
        });
    }

    if !app.finder.results.is_empty() {
        ui.add_space(10.0);
        card().show(ui, |ui| {
            section_label(ui, "RESULTS");
            ui.add_space(6.0);
            let mut ranked = app.finder.results.clone();
            ranked.sort_by(|a, b| {
                let a_reliable =
                    a.alignment.reliable && a.alignment.span_s >= MIN_TRIAL_DATA_SECONDS;
                let b_reliable =
                    b.alignment.reliable && b.alignment.span_s >= MIN_TRIAL_DATA_SECONDS;
                b_reliable.cmp(&a_reliable).then_with(|| {
                    b.alignment
                        .score
                        .partial_cmp(&a.alignment.score)
                        .unwrap()
                })
            });
            let best_rate = ranked
                .iter()
                .find(|result| {
                    result.alignment.reliable
                        && result.alignment.span_s >= MIN_TRIAL_DATA_SECONDS
                })
                .map(|result| result.rate);
            for result in ranked {
                let value = result.alignment;
                let (status, tint) = if value.span_s < MIN_TRIAL_DATA_SECONDS {
                    ("PARTIAL", AMBER)
                } else if Some(result.rate) == best_rate {
                    ("BEST CANDIDATE", ACCENT)
                } else if value.reliable {
                    ("CANDIDATE", BLUE)
                } else {
                    ("NO MATCH", AMBER)
                };
                ui.label(
                    egui::RichText::new(format!("{:.1} breaths/min", result.rate))
                        .size(15.0)
                        .strong()
                        .color(TEXT),
                );
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(status)
                            .size(10.0)
                            .strong()
                            .color(tint),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "Score {:.0} · peak {:.1} bpm · {:.1} bpm from pace · {:.0}s clean data",
                            value.score, value.peak_bpm, value.mismatch_bpm, value.span_s
                        ))
                        .size(11.0)
                        .color(MUTED),
                    );
                });
                ui.add_space(4.0);
            }
            ui.label(
                egui::RichText::new(match best_rate {
                    Some(rate) => format!(
                        "Best response: {rate:.1} breaths/min. Confirm with respiration monitoring or a longer session."
                    ),
                    None => "No reliable response yet. Repeat with steady breathing and a stable sensor.".to_owned(),
                })
                .size(12.0)
                .color(MUTED),
            );
            ui.label(
                egui::RichText::new(
                    "Candidate = score ≥35 with a heart-rhythm peak within 0.5 bpm of the paced rate. Breathing itself is not measured.",
                )
                .size(11.0)
                .color(MUTED),
            );
        });
    }
}
