//! The transport bar: Return to Zero, Stop, Play, Record, Cycle; the position in bars|beats and
//! min:sec; the tempo and meter; the main meter. Icons are drawn in code (no assets).

use crate::StudioApp;
use crate::theme::{TOKENS, TRANSPORT_H};
use egui::{Align2, Color32, FontId, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_time::Samples;

#[derive(Clone, Copy)]
enum Icon {
    Rtz,
    Stop,
    Play,
    Record,
    Cycle,
}

fn icon(p: &egui::Painter, r: Rect, which: Icon, color: Color32) {
    let c = r.center();
    let s = 6.0;
    match which {
        Icon::Play => {
            p.add(Shape::convex_polygon(vec![pos2(c.x - s * 0.8, c.y - s), pos2(c.x + s, c.y), pos2(c.x - s * 0.8, c.y + s)], color, Stroke::NONE));
        }
        Icon::Stop => {
            p.rect_filled(Rect::from_center_size(c, vec2(s * 1.7, s * 1.7)), 1.0, color);
        }
        Icon::Record => {
            p.circle_filled(c, s, color);
        }
        Icon::Rtz => {
            p.rect_filled(Rect::from_center_size(pos2(c.x - s, c.y), vec2(2.0, s * 2.0)), 0.0, color);
            p.add(Shape::convex_polygon(vec![pos2(c.x + s, c.y - s), pos2(c.x + s, c.y + s), pos2(c.x - s + 2.0, c.y)], color, Stroke::NONE));
        }
        Icon::Cycle => {
            let r1 = Rect::from_center_size(c, vec2(s * 2.6, s * 1.5));
            p.rect_stroke(r1, 4.0, Stroke::new(1.6, color), egui::StrokeKind::Middle);
            p.add(Shape::convex_polygon(
                vec![pos2(r1.max.x - 3.0, r1.min.y - 3.0), pos2(r1.max.x + 2.0, r1.min.y), pos2(r1.max.x - 3.0, r1.min.y + 3.0)],
                color,
                Stroke::NONE,
            ));
        }
    }
}

/// A square transport button; returns whether it was clicked.
fn button(app: &mut StudioApp, ui: &mut Ui, name: &str, which: Icon, on: bool, on_color: Color32) -> bool {
    let (r, resp) = ui.allocate_exact_size(vec2(34.0, 28.0), Sense::click());
    let p = ui.painter();
    let bg = if on {
        on_color.gamma_multiply(0.35)
    } else if resp.hovered() {
        TOKENS.button_hover
    } else {
        TOKENS.button
    };
    p.rect_filled(r, 4.0, bg);
    icon(p, r, which, if on { on_color } else { TOKENS.text });
    app.arrange.layout.transport.insert(name.to_string(), [r.min.x, r.min.y, r.max.x, r.max.y]);
    resp.on_hover_text(name).clicked()
}

/// `1. 1. 1. 000` (bar, beat, sixteenth, tick within the sixteenth).
pub fn bars_beats(app: &StudioApp, at: Samples) -> String {
    let s = app.engine.session();
    let bb = s.tempo.bar_beat_at(at.max(0), s.sample_rate);
    let sixteenth = soundcraft_time::TICKS_PER_QUARTER / 4;
    format!("{:>3}. {}. {}. {:03}", bb.bar, bb.beat, bb.tick / sixteenth + 1, bb.tick % sixteenth)
}

/// `0:00.000`.
pub fn min_sec(app: &StudioApp, at: Samples) -> String {
    let secs = at.max(0) as f64 / app.engine.session().sample_rate.as_f64().max(1.0);
    let m = (secs / 60.0).floor();
    format!("{}:{:06.3}", m as i64, secs - m * 60.0)
}

pub fn bar(app: &mut StudioApp, ui: &mut Ui) {
    egui::Panel::top("transport")
        .exact_size(TRANSPORT_H)
        .frame(egui::Frame::NONE.fill(TOKENS.panel_raised).inner_margin(egui::Margin::symmetric(10, 6)))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let playing = app.is_playing();
                let recording = app.engine.transport.recording;
                let cycle = app.engine.session().edit.loop_playback;
                if button(app, ui, "rtz", Icon::Rtz, false, TOKENS.accent) {
                    let _ = app.run("transport.rtz", json!({}));
                }
                if button(app, ui, "stop", Icon::Stop, false, TOKENS.accent) {
                    let _ = app.run("transport.stop", json!({}));
                }
                if button(app, ui, "play", Icon::Play, playing && !recording, TOKENS.accent) {
                    let _ = app.run("transport.play", json!({}));
                }
                if button(app, ui, "record", Icon::Record, recording, TOKENS.record) {
                    let _ = app.run("transport.record", json!({}));
                }
                if button(app, ui, "cycle", Icon::Cycle, cycle, TOKENS.accent) {
                    let _ = app.run("options.loop_playback", json!({"value": !cycle}));
                }
                ui.add_space(14.0);
                let at = app.position();
                readout(ui, &bars_beats(app, at), 128.0, TOKENS.text);
                readout(ui, &min_sec(app, at), 92.0, TOKENS.text_dim);
                ui.add_space(14.0);
                let s = app.engine.session();
                let tick = s.tempo.samples_to_ticks(at.max(0), s.sample_rate);
                let bpm = s.tempo.tempo_at_tick(tick);
                let m = s.tempo.meter_at_tick(tick);
                readout(ui, &format!("{bpm:.2} BPM"), 92.0, TOKENS.text);
                readout(ui, &format!("{}/{}", m.numerator, m.denominator), 40.0, TOKENS.text);
                ui.add_space(14.0);
                main_meter(app, ui);
                if let Some(hint) = app.record_hint() {
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new(hint).color(TOKENS.record));
                }
            });
        });
}

fn readout(ui: &mut Ui, text: &str, w: f32, color: Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
    ui.painter().rect_filled(r, 3.0, TOKENS.bg);
    ui.painter().text(r.center(), Align2::CENTER_CENTER, text, FontId::monospace(13.0), color);
}

fn main_meter(app: &StudioApp, ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(120.0, 16.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(r, 2.0, TOKENS.bg);
    for (i, lvl) in app.main_meter.level.iter().enumerate() {
        let h = (r.height() - 3.0) / 2.0;
        let y = r.min.y + 1.0 + i as f32 * (h + 1.0);
        let db = if *lvl > 0.0 { 20.0 * lvl.log10() } else { -90.0 };
        let k = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        let bar = Rect::from_min_size(pos2(r.min.x + 1.0, y), vec2((r.width() - 2.0) * k, h));
        p.rect_filled(bar, 0.0, if app.main_meter.clip { TOKENS.record } else { TOKENS.monitor });
    }
}
