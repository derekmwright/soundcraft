//! The arrangement window: a track list on the left, a bar/beat ruler, and the timeline with
//! clips as colored regions. Clicking a clip selects it; dragging moves it (Alt-drag copies,
//! Ctrl-drag keeps it on its track); dragging an edge trims it; clicking empty timeline sets the
//! cursor. Every edit goes through engine commands (see `actions`).

use crate::StudioApp;
use crate::theme::{self, CLIP_NAME_H, RULER_H, TOKENS};
use egui::{Align2, Color32, CursorIcon, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_model::{Clip, ClipContent, ClipId, Session, Track, TrackId, TrackKind};
use soundcraft_time::{BarBeat, Samples};

/// How close (points) to a clip edge a press trims instead of moving.
const EDGE: f32 = 6.0;

/// What a drag on a clip does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum DragKind {
    Move,
    TrimStart,
    TrimEnd,
}

/// A mouse gesture in progress.
#[derive(Debug, Clone)]
pub enum Gesture {
    Clip { clip: ClipId, track: TrackId, kind: DragKind, press: Pos2, start: Samples, end: Samples },
    Volume { track: TrackId, start_db: f32 },
}

/// Rects as `[x0, y0, x1, y1]`, for `ui.inspect` (scripts drive the UI with them).
pub type R4 = [f32; 4];

fn r4(r: Rect) -> R4 {
    [r.min.x, r.min.y, r.max.x, r.max.y]
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct TrackRow {
    pub id: u64,
    pub name: String,
    pub kind: TrackKind,
    pub header: R4,
    pub lane: R4,
    pub mute: R4,
    pub solo: R4,
    pub arm: R4,
    pub monitor: R4,
    pub volume: R4,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ClipRect {
    pub id: u64,
    pub track: u64,
    pub rect: R4,
    pub start: Samples,
    pub end: Samples,
}

/// Last frame's arrangement geometry.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Layout {
    pub track_list: R4,
    pub ruler: R4,
    pub timeline: R4,
    pub samples_per_px: f64,
    pub scroll: Samples,
    pub tracks: Vec<TrackRow>,
    pub clips: Vec<ClipRect>,
    pub add_audio: R4,
    pub add_instrument: R4,
    /// Transport bar buttons (`play`, `stop`, `record`, `rtz`, `cycle`).
    pub transport: std::collections::BTreeMap<String, R4>,
}

#[derive(Debug, Default)]
pub struct ArrangeState {
    pub gesture: Option<Gesture>,
    /// Vertical scroll of the track rows (points).
    pub vscroll: f32,
    pub layout: Layout,
    /// The track menu's open "Instrument" submenu entries (`value` is the plug-in id).
    pub menu: Vec<crate::mixer::MenuItem>,
}

/// Timeline mapping: x ↔ samples, from the session's shared zoom (`edit.zoom`).
#[derive(Clone, Copy)]
struct Map {
    x0: f32,
    scroll: Samples,
    spp: f64,
}

impl Map {
    fn x(&self, at: Samples) -> f32 {
        self.x0 + ((at - self.scroll) as f64 / self.spp) as f32
    }
    fn at(&self, x: f32) -> Samples {
        self.scroll + (f64::from(x - self.x0) * self.spp).round() as Samples
    }
}

fn shown(s: &Session, t: &Track) -> bool {
    !t.hidden && t.folder.and_then(|f| s.track(f)).is_none_or(|f| f.folder_open)
}

pub fn show(app: &mut StudioApp, ui: &mut Ui) {
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(TOKENS.bg)).show(ui, |ui| arrangement(app, ui));
}

fn arrangement(app: &mut StudioApp, ui: &mut Ui) {
    let full = ui.max_rect();
    let hw = app.ui.header_width.clamp(160.0, 480.0);
    let rh = app.ui.track_height.clamp(24.0, 200.0);
    let list = Rect::from_min_max(pos2(full.min.x, full.min.y + RULER_H), pos2(full.min.x + hw, full.max.y));
    let ruler = Rect::from_min_max(pos2(full.min.x + hw, full.min.y), pos2(full.max.x, full.min.y + RULER_H));
    let tl = Rect::from_min_max(pos2(full.min.x + hw, full.min.y + RULER_H), full.max);
    let corner = Rect::from_min_max(full.min, pos2(full.min.x + hw, full.min.y + RULER_H));
    let s = app.engine.session_arc();
    let map = Map { x0: tl.min.x, scroll: s.edit.zoom.scroll, spp: s.edit.zoom.samples_per_px.max(0.01) };
    let p = ui.painter();
    p.rect_filled(corner, 0.0, TOKENS.ruler);
    p.rect_filled(list, 0.0, TOKENS.panel);

    // Rows.
    let rows: Vec<&Track> = s.tracks.iter().filter(|t| shown(&s, t)).collect();
    let content_h = rows.len() as f32 * rh + 60.0;
    app.arrange.vscroll = app.arrange.vscroll.clamp(0.0, (content_h - tl.height()).max(0.0));
    let mut layout = Layout {
        track_list: r4(list),
        ruler: r4(ruler),
        timeline: r4(tl),
        samples_per_px: map.spp,
        scroll: map.scroll,
        transport: std::mem::take(&mut app.arrange.layout.transport),
        ..Layout::default()
    };
    let selected_tracks = s.edit.selected_tracks.clone();
    let focus = app.focus_track();
    app.arrange.menu.clear();
    for (i, t) in rows.iter().enumerate() {
        let y0 = tl.min.y + i as f32 * rh - app.arrange.vscroll;
        let header = Rect::from_min_max(pos2(list.min.x, y0), pos2(list.max.x, y0 + rh));
        let lane = Rect::from_min_max(pos2(tl.min.x, y0), pos2(tl.max.x, y0 + rh));
        let mut row = TrackRow { id: t.id.0, name: t.name.clone(), kind: t.kind, header: r4(header), lane: r4(lane), ..TrackRow::default() };
        if header.max.y >= list.min.y && header.min.y <= list.max.y {
            let selected = selected_tracks.contains(&t.id) || focus == Some(t.id);
            track_header(app, ui, t, header, list, selected, &mut row);
        }
        let lane_fill = if i % 2 == 0 { TOKENS.lane } else { TOKENS.lane_alt };
        ui.painter().with_clip_rect(tl).rect_filled(lane, 0.0, lane_fill);
        layout.tracks.push(row);
    }
    // Add Track buttons below the rows.
    let below = tl.min.y + rows.len() as f32 * rh - app.arrange.vscroll + 8.0;
    let add_a = Rect::from_min_size(pos2(list.min.x + 8.0, below), vec2((hw - 24.0) / 2.0, 22.0));
    let add_i = Rect::from_min_size(pos2(add_a.max.x + 8.0, below), add_a.size());
    if add_a.min.y < list.max.y {
        let clip = ui.painter().with_clip_rect(list);
        for (r, label, kind, salt) in [(add_a, "+ Audio", "audio", "add_audio"), (add_i, "+ Instrument", "instrument", "add_instrument")] {
            let resp = ui.interact(r.intersect(list), ui.id().with(salt), Sense::click());
            clip.rect_filled(r, 3.0, if resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
            clip.text(r.center(), Align2::CENTER_CENTER, label, FontId::proportional(12.0), TOKENS.text);
            if resp.clicked() {
                let _ = app.add_track(kind);
            }
        }
    }
    layout.add_audio = r4(add_a);
    layout.add_instrument = r4(add_i);
    ui.painter().line_segment([pos2(list.max.x, full.min.y), pos2(list.max.x, full.max.y)], Stroke::new(1.0, TOKENS.line));

    grid_and_ruler(app, ui, &s, map, ruler, tl);
    clips(app, ui, &s, map, tl, &rows, rh, &mut layout);
    timeline_input(app, ui, &s, map, tl, &rows, rh, &layout);
    ruler_input(app, ui, map, ruler);
    overlays(app, ui, &s, map, ruler, tl, &rows, rh);
    scroll_and_zoom(app, ui, map, full, tl, content_h);
    follow_playhead(app, map, tl);
    app.arrange.layout = layout;
}

/// One track's header: color strip, name, M S R I toggles, volume readout.
fn track_header(app: &mut StudioApp, ui: &mut Ui, t: &Track, r: Rect, list: Rect, selected: bool, row: &mut TrackRow) {
    let p = ui.painter().with_clip_rect(list);
    let bg = if selected { TOKENS.header_selected } else { TOKENS.header };
    p.rect_filled(r.shrink2(vec2(0.0, 0.5)), 0.0, bg);
    p.rect_filled(Rect::from_min_size(r.min, vec2(5.0, r.height())), 0.0, theme::rgb(t.color));
    p.line_segment([pos2(r.min.x, r.max.y), pos2(r.max.x, r.max.y)], Stroke::new(1.0, TOKENS.line));
    let name_rect = Rect::from_min_max(pos2(r.min.x + 12.0, r.min.y + 4.0), pos2(r.max.x - 92.0, r.min.y + 20.0));
    p.with_clip_rect(name_rect.intersect(list)).text(name_rect.left_center(), Align2::LEFT_CENTER, &t.name, FontId::proportional(13.0), TOKENS.text);
    let kind = match t.kind {
        TrackKind::Instrument => t.instrument.as_ref().map_or("Instrument".to_string(), |i| short_plugin(&i.plugin)),
        TrackKind::Audio => format!("Audio · {}", t.format.label()),
        k => format!("{k:?}"),
    };
    p.text(pos2(r.min.x + 12.0, r.min.y + 30.0), Align2::LEFT_CENTER, kind, FontId::proportional(10.5), TOKENS.text_dim);
    // Toggles, right-aligned on the first line.
    let m = &t.mixer;
    let mut x = r.max.x - 6.0;
    let mut toggle = |app: &mut StudioApp, label: &str, on: bool, color: Color32, cmd: &str, enabled: bool| -> R4 {
        let b = Rect::from_min_size(pos2(x - 20.0, r.min.y + 5.0), vec2(19.0, 16.0));
        x -= 22.0;
        let resp = ui.interact(b.intersect(list), ui.id().with((cmd, t.id.0)), Sense::click());
        let fill = if on {
            color
        } else if resp.hovered() && enabled {
            TOKENS.button_hover
        } else {
            TOKENS.button
        };
        p.rect_filled(b, 2.0, fill);
        let fg = if on {
            theme::text_on(color)
        } else if enabled {
            TOKENS.text
        } else {
            TOKENS.text_dim.gamma_multiply(0.5)
        };
        p.text(b.center(), Align2::CENTER_CENTER, label, FontId::proportional(11.0), fg);
        if resp.clicked() && enabled {
            let _ = app.run(cmd, json!({"tracks": [t.id.0], "value": !on}));
        }
        r4(b)
    };
    let records = t.kind.has_playlist();
    row.monitor = toggle(app, "I", m.input_monitor, TOKENS.monitor, "mix.input_monitor", t.kind == TrackKind::Audio);
    row.arm = toggle(app, "R", m.record_arm, TOKENS.record, "mix.record_arm", records);
    row.solo = toggle(app, "S", m.solo, TOKENS.solo, "mix.solo", true);
    row.mute = toggle(app, "M", m.mute, TOKENS.mute, "mix.mute", true);
    // Volume readout: drag sideways to change, double-click for 0 dB.
    let vol = Rect::from_min_size(pos2(r.max.x - 72.0, r.min.y + 23.0), vec2(66.0, 15.0));
    row.volume = r4(vol);
    let resp = ui.interact(vol.intersect(list), ui.id().with(("vol", t.id.0)), Sense::click_and_drag());
    p.rect_filled(vol, 2.0, TOKENS.bg);
    let db = m.volume_db;
    let text = if db <= -144.0 { "-inf dB".to_string() } else { format!("{db:+.1} dB") };
    p.text(vol.center(), Align2::CENTER_CENTER, text, FontId::monospace(10.5), if resp.hovered() { TOKENS.accent } else { TOKENS.text });
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
    if resp.drag_started() {
        app.arrange.gesture = Some(Gesture::Volume { track: t.id, start_db: db });
    }
    if resp.dragged()
        && let Some(Gesture::Volume { track, start_db }) = app.arrange.gesture.clone()
        && track == t.id
    {
        let dx = ui.input(|i| i.pointer.press_origin().zip(i.pointer.latest_pos()).map_or(0.0, |(a, b)| b.x - a.x));
        let fine = ui.input(|i| i.modifiers.shift);
        let target = (start_db + dx * if fine { 0.02 } else { 0.1 }).clamp(-80.0, 12.0);
        let _ = app.engine.execute_merged("mix.volume", &json!({"tracks": [t.id.0], "db": target}), &format!("fader:{}", t.id.0));
    }
    if resp.drag_stopped() {
        app.arrange.gesture = None;
    }
    if resp.double_clicked() {
        let _ = app.run("mix.volume", json!({"tracks": [t.id.0], "db": 0.0}));
    }
    // Meter: a thin bar at the right edge.
    if let Some(d) = app.meters.get(&t.id) {
        let lvl = d.level[0].max(d.level[1]).clamp(0.0, 1.0);
        let h = (r.height() - 4.0) * meter_pos(lvl);
        let bar = Rect::from_min_max(pos2(r.max.x - 3.0, r.max.y - 2.0 - h), pos2(r.max.x - 1.0, r.max.y - 2.0));
        p.rect_filled(bar, 0.0, if d.clip { TOKENS.record } else { TOKENS.monitor });
    }
    // Name area: click selects the track; right-click opens the track menu.
    let name_area = Rect::from_min_max(r.min, pos2(r.max.x - 92.0, r.max.y));
    let resp = ui.interact(name_area.intersect(list), ui.id().with(("hdr", t.id.0)), Sense::click());
    if resp.clicked() {
        app.focus(t.id);
    }
    let id = t.id;
    let kind = t.kind;
    let has_instrument = t.instrument.is_some();
    resp.context_menu(|ui| track_menu(app, ui, id, kind, has_instrument));
}

/// The track's right-click menu.
fn track_menu(app: &mut StudioApp, ui: &mut Ui, id: TrackId, kind: TrackKind, has_instrument: bool) {
    if kind == TrackKind::Instrument {
        // Instrument ▸: the device chain's instrument menu (a click in its filter box keeps it open).
        let config = egui::containers::menu::MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
        let mut picked = None;
        let (button, _) = egui::containers::menu::SubMenuButton::new("Instrument").config(config).ui(ui, |ui| {
            let lists = app.instrument_lists();
            let current = app.engine.session().track(id).and_then(|t| t.instrument.as_ref()).map(|i| i.plugin.clone());
            let mut filter = std::mem::take(&mut app.mixer.instrument_filter);
            let mut filter_rect = app.mixer.filter_rect;
            let mut items = Vec::new();
            picked = crate::devices::instrument_menu(ui, &lists, current.as_deref(), &mut filter, &mut filter_rect, &mut items);
            app.mixer.instrument_filter = filter;
            app.mixer.filter_rect = filter_rect;
            app.arrange.menu.extend(items);
        });
        app.arrange.menu.push(crate::mixer::MenuItem { label: "Instrument".into(), value: String::new(), rect: r4(button.rect), visible: true });
        if let Some(plugin) = picked {
            app.mixer.instrument_filter.clear();
            let _ = app.set_instrument(id, &plugin);
            ui.close();
        }
        if ui.add_enabled(has_instrument && app.player.is_some(), egui::Button::new("Open Instrument Editor")).clicked() {
            app.want_editor((id, soundcraft_mix::INSTRUMENT_SLOT));
            ui.close();
        }
        ui.separator();
    }
    if kind.is_midi() && ui.button("New MIDI Clip at Cursor").clicked() {
        let _ = app.new_midi_clip(Some(id));
        ui.close();
    }
    ui.menu_button("Color", |ui| {
        for (i, c) in PALETTE.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(120.0, 16.0), Sense::click());
            ui.painter().rect_filled(r.shrink(1.0), 2.0, theme::rgb(*c));
            if resp.clicked() {
                let _ = app.run("track.color", json!({"tracks": [id.0], "color": c}));
                ui.close();
            }
            let _ = i;
        }
    });
    ui.separator();
    if ui.button("Delete Track").clicked() {
        let _ = app.run("track.delete", json!({"tracks": [id.0]}));
        ui.close();
    }
}

/// Track colors offered in the track menu (muted, distinguishable on the dark lanes).
const PALETTE: [[u8; 3]; 10] = [
    [0xd9, 0x5f, 0x5f],
    [0xe0, 0x8c, 0x4a],
    [0xd6, 0xbe, 0x4f],
    [0x8f, 0xc0, 0x5a],
    [0x4f, 0xb8, 0x8c],
    [0x4a, 0xb4, 0xc8],
    [0x5b, 0x8f, 0xdb],
    [0x8e, 0x74, 0xd6],
    [0xc7, 0x6a, 0xc0],
    [0x9a, 0x9c, 0xa3],
];

fn short_plugin(id: &str) -> String {
    let name = id.rsplit(':').next().unwrap_or(id);
    if name.len() > 24 { format!("{}…", name.chars().take(22).collect::<String>()) } else { name.to_string() }
}

/// 0..1 meter position for a linear peak (-60 dB at the bottom).
fn meter_pos(lin: f32) -> f32 {
    if lin <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * lin.log10();
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}

fn bar_at(s: &Session, bar: i64) -> Samples {
    s.tempo.samples_at_bar_beat(BarBeat { bar, beat: 1, tick: 0 }, s.sample_rate)
}

/// Bar and beat lines across the timeline, and the ruler with bar numbers.
fn grid_and_ruler(app: &StudioApp, ui: &Ui, s: &Session, map: Map, ruler: Rect, tl: Rect) {
    let p = ui.painter();
    p.rect_filled(ruler, 0.0, TOKENS.ruler);
    p.line_segment([pos2(ruler.min.x, ruler.max.y), pos2(ruler.max.x, ruler.max.y)], Stroke::new(1.0, TOKENS.line));
    let first = s.tempo.bar_beat_at(map.at(tl.min.x).max(0), s.sample_rate).bar.max(1);
    let bar_px = (map.x(bar_at(s, first + 1)) - map.x(bar_at(s, first))).max(0.01);
    // Label every n bars so labels keep about 40 points apart.
    let every = [1, 2, 4, 8, 16, 32, 64, 128].into_iter().find(|n| bar_px * *n as f32 >= 40.0).unwrap_or(256);
    let gp = p.with_clip_rect(tl);
    let rp = p.with_clip_rect(ruler);
    for bar in (first..).take(4000) {
        let x = map.x(bar_at(s, bar));
        if x > tl.max.x {
            break;
        }
        let beats = s.tempo.meter_at_tick(s.tempo.bar_start_tick(bar)).numerator.max(1);
        let next = map.x(bar_at(s, bar + 1));
        if (next - x) / beats as f32 >= 10.0 {
            for b in 1..beats {
                let bx = map.x(s.tempo.samples_at_bar_beat(BarBeat { bar, beat: i64::from(b) + 1, tick: 0 }, s.sample_rate));
                gp.line_segment([pos2(bx, tl.min.y), pos2(bx, tl.max.y)], Stroke::new(1.0, TOKENS.grid_beat));
                rp.line_segment([pos2(bx, ruler.max.y - 5.0), pos2(bx, ruler.max.y)], Stroke::new(1.0, TOKENS.text_dim.gamma_multiply(0.5)));
            }
        }
        if (bar - 1) % every == 0 {
            gp.line_segment([pos2(x, tl.min.y), pos2(x, tl.max.y)], Stroke::new(1.0, TOKENS.grid_bar));
            rp.line_segment([pos2(x, ruler.min.y + 10.0), pos2(x, ruler.max.y)], Stroke::new(1.0, TOKENS.text_dim));
            rp.text(pos2(x + 4.0, ruler.min.y + 9.0), Align2::LEFT_CENTER, bar.to_string(), FontId::proportional(11.0), TOKENS.text);
        } else if bar_px >= 6.0 {
            gp.line_segment([pos2(x, tl.min.y), pos2(x, tl.max.y)], Stroke::new(1.0, TOKENS.grid_beat));
        }
    }
    let _ = app;
}

/// The clip being dragged, as it would land: (track, start, end).
fn drag_preview(app: &StudioApp, ui: &Ui, map: Map, rows: &[&Track], tl: Rect, rh: f32) -> Option<(ClipId, TrackId, Samples, Samples, bool)> {
    let Some(Gesture::Clip { clip, track, kind, press, start, end }) = app.arrange.gesture.clone() else { return None };
    let (pos, m) = ui.input(|i| (i.pointer.latest_pos(), i.modifiers));
    let pos = pos?;
    let dx = f64::from(pos.x - press.x) * map.spp;
    let dx = dx.round() as Samples;
    Some(match kind {
        DragKind::Move => {
            let new_start = (start + dx).max(0);
            let row = ((pos.y - tl.min.y + app.arrange.vscroll) / rh).floor();
            let dest = if m.command || m.ctrl || row < 0.0 {
                track
            } else {
                rows.get(row as usize).map(|t| t.id).filter(|t| app.holds_clips(*t)).unwrap_or(track)
            };
            (clip, dest, new_start, new_start + (end - start), m.alt)
        }
        DragKind::TrimStart => (clip, track, (start + dx).clamp(0, end - 1), end, false),
        DragKind::TrimEnd => (clip, track, start, (end + dx).max(start + 1), false),
    })
}

/// Clips as solid regions in the track color, with a name strip and their content.
#[allow(clippy::too_many_arguments)]
fn clips(app: &StudioApp, ui: &Ui, s: &Session, map: Map, tl: Rect, rows: &[&Track], rh: f32, layout: &mut Layout) {
    let p = ui.painter().with_clip_rect(tl);
    let sel = &s.edit.selected_clips;
    let preview = drag_preview(app, ui, map, rows, tl, rh);
    let row_y = |t: TrackId| rows.iter().position(|r| r.id == t).map(|i| tl.min.y + i as f32 * rh - app.arrange.vscroll);
    for t in rows {
        let Some(y0) = row_y(t.id) else { continue };
        for c in t.clips() {
            let rect = Rect::from_min_max(pos2(map.x(c.start), y0 + 1.0), pos2(map.x(c.end()).max(map.x(c.start) + 2.0), y0 + rh - 1.0));
            layout.clips.push(ClipRect { id: c.id.0, track: t.id.0, rect: r4(rect), start: c.start, end: c.end() });
            if rect.max.x < tl.min.x || rect.min.x > tl.max.x || rect.max.y < tl.min.y || rect.min.y > tl.max.y {
                continue;
            }
            let moving = preview.is_some_and(|(id, _, _, _, copy)| id == c.id && !copy);
            draw_clip(&p, s, map, tl, t, c, rect, sel.contains(&c.id), if moving { 0.35 } else { 1.0 });
        }
    }
    // The dragged clip where it would land.
    if let Some((id, dest, a, b, _)) = preview
        && let (Some((_, c)), Some(y0)) = (s.find_clip(id), row_y(dest))
        && let Some(t) = s.track(dest)
    {
        let rect = Rect::from_min_max(pos2(map.x(a), y0 + 1.0), pos2(map.x(b).max(map.x(a) + 2.0), y0 + rh - 1.0));
        draw_clip(&p, s, map, tl, t, c, rect, true, 0.85);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_clip(p: &egui::Painter, s: &Session, map: Map, tl: Rect, t: &Track, c: &Clip, rect: Rect, selected: bool, alpha: f32) {
    let base = theme::rgb(c.color.unwrap_or(t.color));
    let base = if c.muted { theme::darken(base, 0.5) } else { base };
    let body_col = if selected { theme::lighten(base, 0.12) } else { base };
    let p = p.with_clip_rect(rect.intersect(tl));
    p.rect_filled(rect, 3.0, body_col.gamma_multiply(alpha));
    let strip = Rect::from_min_max(rect.min, pos2(rect.max.x, (rect.min.y + CLIP_NAME_H).min(rect.max.y)));
    p.rect_filled(strip, egui::CornerRadius { nw: 3, ne: 3, sw: 0, se: 0 }, theme::darken(body_col, 0.22).gamma_multiply(alpha));
    let name_x = rect.min.x.max(tl.min.x) + 4.0;
    p.with_clip_rect(strip.intersect(tl)).text(
        pos2(name_x, strip.center().y),
        Align2::LEFT_CENTER,
        &c.name,
        FontId::proportional(11.0),
        theme::text_on(theme::darken(body_col, 0.22)),
    );
    let body = Rect::from_min_max(pos2(rect.min.x, strip.max.y), rect.max).shrink2(vec2(0.0, 2.0));
    let ink = theme::darken(body_col, 0.62).gamma_multiply(alpha);
    if body.height() > 4.0 {
        match &c.content {
            ClipContent::Audio { source, .. } => {
                if let Some(audio) = s.pool.get(*source) {
                    let wp = p.with_clip_rect(body.intersect(tl));
                    crate::waveform::draw(&wp, s, tl, c, audio, t.channels().min(2), body, ink);
                }
            }
            ClipContent::Midi { sequence } => {
                let notes = &sequence.notes;
                if let (Some(lo), Some(hi)) = (notes.iter().map(|n| n.pitch).min(), notes.iter().map(|n| n.pitch).max()) {
                    let span = f32::from(hi.saturating_sub(lo)).max(11.0);
                    let clip_tick = s.tempo.samples_to_ticks(c.start, s.sample_rate);
                    for n in notes.iter().take(5000) {
                        let a = s.tempo.tick_to_samples(clip_tick.saturating_add(n.start), s.sample_rate);
                        let b = s.tempo.tick_to_samples(clip_tick.saturating_add(n.end()), s.sample_rate);
                        let fy = 1.0 - f32::from(n.pitch.saturating_sub(lo)) / span;
                        let y = body.min.y + 2.0 + fy * (body.height() - 6.0);
                        let r = Rect::from_min_max(pos2(map.x(a), y), pos2(map.x(b).max(map.x(a) + 2.0), y + 2.5));
                        p.rect_filled(r, 0.0, ink);
                    }
                }
            }
            ClipContent::Video { .. } => {}
        }
    }
    let stroke = if selected { Stroke::new(2.0, TOKENS.selection_outline) } else { Stroke::new(1.0, theme::darken(base, 0.45)) };
    p.rect_stroke(rect, 3.0, stroke, StrokeKind::Inside);
}

/// What is under a point: a clip (and whether near its start or end edge) and the row's track.
fn hit(layout: &Layout, pos: Pos2) -> (Option<(ClipId, TrackId, DragKind, Samples, Samples)>, Option<TrackId>) {
    let track = layout.tracks.iter().find(|r| pos.y >= r.lane[1] && pos.y < r.lane[3]).map(|r| TrackId(r.id));
    // Topmost (last drawn) first.
    let clip = layout.clips.iter().rev().find(|c| pos.x >= c.rect[0] && pos.x <= c.rect[2] && pos.y >= c.rect[1] && pos.y <= c.rect[3]).map(|c| {
        let w = c.rect[2] - c.rect[0];
        let edge = EDGE.min(w / 4.0);
        let kind = if pos.x - c.rect[0] <= edge {
            DragKind::TrimStart
        } else if c.rect[2] - pos.x <= edge {
            DragKind::TrimEnd
        } else {
            DragKind::Move
        };
        (ClipId(c.id), TrackId(c.track), kind, c.start, c.end)
    });
    (clip, track)
}

#[allow(clippy::too_many_arguments)]
fn timeline_input(app: &mut StudioApp, ui: &mut Ui, s: &Session, map: Map, tl: Rect, rows: &[&Track], rh: f32, layout: &Layout) {
    let resp = ui.interact(tl, ui.id().with("timeline"), Sense::click_and_drag());
    if let Some(pos) = resp.hover_pos()
        && app.arrange.gesture.is_none()
    {
        match hit(layout, pos).0 {
            Some((_, _, DragKind::TrimStart | DragKind::TrimEnd, _, _)) => ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal),
            Some(_) => ui.ctx().set_cursor_icon(CursorIcon::Grab),
            None => {}
        }
    }
    if resp.drag_started()
        && let Some(press) = ui.input(|i| i.pointer.press_origin())
        && let (Some((clip, track, kind, start, end)), _) = hit(layout, press)
    {
        if !s.edit.selected_clips.contains(&clip) {
            app.click_clip(clip, false);
        }
        app.arrange.gesture = Some(Gesture::Clip { clip, track, kind, press, start, end });
    }
    if resp.dragged() && matches!(app.arrange.gesture, Some(Gesture::Clip { kind: DragKind::Move, .. })) {
        let copy = ui.input(|i| i.modifiers.alt);
        ui.ctx().set_cursor_icon(if copy { CursorIcon::Copy } else { CursorIcon::Grabbing });
    }
    if resp.drag_stopped() {
        let preview = drag_preview(app, ui, map, rows, tl, rh);
        let gesture = app.arrange.gesture.take();
        if let (Some(Gesture::Clip { kind, track, start, end, .. }), Some((clip, dest, a, b, copy))) = (gesture, preview) {
            commit_drag(app, kind, clip, track, dest, start, end, a, b, copy);
        }
    }
    if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let additive = ui.input(|i| i.modifiers.shift || i.modifiers.command);
        match hit(layout, pos) {
            (Some((clip, ..)), _) => app.click_clip(clip, additive),
            (None, track) => app.click_empty(track, map.at(pos.x)),
        }
    }
    if resp.double_clicked()
        && let Some(pos) = resp.interact_pointer_pos()
        && let (None, Some(track)) = hit(layout, pos)
        && s.track(track).is_some_and(|t| t.kind.is_midi())
    {
        let _ = app.new_midi_clip(Some(track));
    }
    resp.context_menu(|ui| {
        if ui.button("Add Audio Track").clicked() {
            let _ = app.add_track("audio");
            ui.close();
        }
        if ui.button("Add Instrument Track").clicked() {
            let _ = app.add_track("instrument");
            ui.close();
        }
        ui.separator();
        if ui.button("Paste at Cursor").clicked() {
            let _ = app.paste_at_cursor();
            ui.close();
        }
        if ui.button("Split at Cursor").clicked() {
            let _ = app.split_at_cursor();
            ui.close();
        }
    });
}

/// Turn a finished drag into commands.
#[allow(clippy::too_many_arguments)]
fn commit_drag(
    app: &mut StudioApp,
    kind: DragKind,
    clip: ClipId,
    from: TrackId,
    dest: TrackId,
    start: Samples,
    end: Samples,
    a: Samples,
    b: Samples,
    copy: bool,
) {
    match kind {
        DragKind::Move => {
            let by = a - start;
            if by == 0 && dest == from {
                if copy {
                    app.status = "Alt-drag the clip somewhere else to copy it.".into();
                }
                return;
            }
            if copy {
                let _ = app.copy_clip_to(clip, a, dest);
                return;
            }
            let sel: Vec<ClipId> = app.selected_clips().into_iter().map(|(_, c, _)| c).collect();
            let set = if sel.contains(&clip) && sel.len() > 1 { sel } else { vec![clip] };
            // Several clips move in time only; one clip may change track.
            let to = (set.len() == 1 && dest != from).then_some(dest);
            let _ = app.move_clips(&set, by, to);
        }
        DragKind::TrimStart if a != start => {
            let _ = app.trim_clip(clip, true, a);
        }
        DragKind::TrimEnd if b != end => {
            let _ = app.trim_clip(clip, false, b);
        }
        _ => {}
    }
}

fn ruler_input(app: &mut StudioApp, ui: &mut Ui, map: Map, ruler: Rect) {
    let resp = ui.interact(ruler, ui.id().with("ruler"), Sense::click_and_drag());
    if (resp.clicked() || resp.dragged())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        app.click_ruler(map.at(pos.x.clamp(ruler.min.x, ruler.max.x)));
    }
}

/// Cycle range, recording takes, cursor and playhead.
#[allow(clippy::too_many_arguments)]
fn overlays(app: &StudioApp, ui: &Ui, s: &Session, map: Map, ruler: Rect, tl: Rect, rows: &[&Track], rh: f32) {
    let p = ui.painter();
    let sel = s.edit.selection;
    if s.edit.loop_playback && !sel.is_empty() {
        let r = Rect::from_min_max(pos2(map.x(sel.start), ruler.min.y + 14.0), pos2(map.x(sel.end), ruler.max.y - 1.0)).intersect(ruler);
        p.rect_filled(r, 2.0, TOKENS.accent.gamma_multiply(0.55));
    }
    if app.engine.transport.recording {
        let rp = p.with_clip_rect(tl);
        for (i, t) in rows.iter().enumerate() {
            if t.mixer.record_arm {
                let y0 = tl.min.y + i as f32 * rh - app.arrange.vscroll;
                let r = Rect::from_min_max(pos2(map.x(app.record_start), y0 + 1.0), pos2(map.x(app.position()), y0 + rh - 1.0));
                rp.rect_filled(r, 3.0, TOKENS.record.gamma_multiply(0.45));
            }
        }
    }
    let full = Rect::from_min_max(ruler.min, tl.max);
    let lp = p.with_clip_rect(full);
    let cx = map.x(app.cursor());
    lp.line_segment([pos2(cx, ruler.min.y), pos2(cx, tl.max.y)], Stroke::new(1.0, TOKENS.cursor.gamma_multiply(0.8)));
    let tri = vec![pos2(cx - 5.0, ruler.max.y - 8.0), pos2(cx + 5.0, ruler.max.y - 8.0), pos2(cx, ruler.max.y - 1.0)];
    lp.add(egui::Shape::convex_polygon(tri, TOKENS.cursor, Stroke::NONE));
    if app.is_playing() {
        let px = map.x(app.engine.transport.position);
        lp.line_segment([pos2(px, ruler.min.y), pos2(px, tl.max.y)], Stroke::new(1.5, TOKENS.accent));
    }
}

/// Wheel scrolls the rows, Shift+wheel or a sideways swipe scrolls time, Ctrl+wheel or a pinch zooms.
fn scroll_and_zoom(app: &mut StudioApp, ui: &Ui, map: Map, full: Rect, tl: Rect, content_h: f32) {
    let Some(pos) = ui.input(|i| i.pointer.hover_pos()) else { return };
    if !full.contains(pos) {
        return;
    }
    let (delta, zoom, shift) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers.shift));
    if (zoom - 1.0).abs() > 1e-4 {
        let _ = app.engine.execute("view.zoom_by", &json!({"factor": zoom, "x": (pos.x - tl.min.x).max(0.0)}));
        return;
    }
    let (dx, dy) = if shift { (delta.y, 0.0) } else { (delta.x, delta.y) };
    if dx.abs() > 0.1 {
        let _ = app.engine.execute("view.scroll", &json!({"by_px": -dx}));
    }
    if dy.abs() > 0.1 {
        app.arrange.vscroll = (app.arrange.vscroll - dy).clamp(0.0, (content_h - tl.height()).max(0.0));
    }
    let _ = map;
}

/// While playing, page the view so the playhead stays on screen.
fn follow_playhead(app: &mut StudioApp, map: Map, tl: Rect) {
    if !app.is_playing() || app.arrange.gesture.is_some() {
        return;
    }
    let x = map.x(app.engine.transport.position);
    if x > tl.max.x - 8.0 || x < tl.min.x {
        let to = app.engine.transport.position - (f64::from(tl.width()) * map.spp * 0.05) as Samples;
        let _ = app.engine.execute("view.scroll", &json!({"to": to.max(0)}));
    }
}
