//! The device chain of the focused track, under the arrangement (Ableton's detail view): the
//! instrument first on instrument tracks, then the occupied insert slots as cards in slot order,
//! then "+", which lists the built-in effects and the scanned VST3 and CLAP effects.
//!
//! Plan 06 leaks: inserts are ten fixed slots (L1), so the chain shows only the occupied ones and
//! "+" uses `mix.insert` without a slot (the first free one); an eleventh device is refused. There
//! is no drag-reorder: `mix.insert_move` is a swap that leaves slot-keyed automation (L2) and live
//! instances (L3) behind.

use crate::StudioApp;
use crate::mixer::{DeviceCard, DevicesLayout, MenuItem, PluginEntry};
use crate::theme::{self, TOKENS};
use egui::{Align2, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::Value;
use soundcraft_model::{Insert, TrackId, TrackKind};

/// Height of the device pane.
pub const PANE_H: f32 = 132.0;
const CARD_W: f32 = 168.0;

fn r4(r: Rect) -> crate::arrange::R4 {
    [r.min.x, r.min.y, r.max.x, r.max.y]
}

/// Whether `id` names a hosted (third-party) plug-in.
fn hosted(id: &str) -> bool {
    id.starts_with("vst3:") || id.starts_with("clap:") || id.starts_with("au:")
}

impl StudioApp {
    /// The built-in plug-ins (`engine.plugins`, a query: no undo step, no document change).
    pub fn builtin_plugins(&mut self) -> Vec<PluginEntry> {
        if self.mixer.builtin.is_none() {
            let list = self.engine.execute("engine.plugins", &serde_json::json!({})).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
            let entries = list
                .iter()
                .map(|p| PluginEntry {
                    id: p.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                    name: p.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                    category: p.get("category").map(|c| c.as_str().map_or_else(|| c.to_string(), str::to_string)).unwrap_or_default(),
                    instrument: p.get("instrument").and_then(Value::as_bool).unwrap_or(false),
                })
                .filter(|p| !p.id.is_empty())
                .collect();
            self.mixer.builtin = Some(entries);
        }
        self.mixer.builtin.clone().unwrap_or_default()
    }

    /// A plug-in's display name: built-in names, hosted names from the scan caches, else the id.
    pub fn plugin_name(&mut self, id: &str) -> String {
        if let Some(n) = self.builtin_plugins().iter().find(|p| p.id == id).map(|p| p.name.clone()) {
            return n;
        }
        if let Some(n) = self.mixer.names.get(id).cloned() {
            return n;
        }
        if hosted(id) {
            let found = soundcraft_vst3_host::cached()
                .into_iter()
                .flatten()
                .map(|d| (d.id, d.name))
                .chain(soundcraft_clap_host::cached().into_iter().flatten().map(|d| (d.id, d.name)))
                .find(|(i, _)| i == id)
                .map(|(_, n)| n);
            if let Some(n) = found {
                self.mixer.names.insert(id.to_string(), n.clone());
                return n;
            }
        }
        id.rsplit(':').next().unwrap_or(id).to_string()
    }
}

/// The "+" menu's hosted effects: `(id, label)`, from the background scan's results only (listing
/// never starts a scan or loads a plug-in; `engine.vst3_plugins` would block on one).
fn hosted_effects() -> (Option<Vec<(String, String)>>, Option<Vec<(String, String)>>) {
    let label = |name: &str, vendor: &str| if vendor.is_empty() { name.to_string() } else { format!("{name} — {vendor}") };
    let vst3 = soundcraft_vst3_host::cached()
        .map(|l| l.into_iter().filter(|d| !d.is_instrument).map(|d| (d.id.clone(), label(&d.name, &d.vendor))).collect());
    let clap = soundcraft_clap_host::cached()
        .map(|l| l.into_iter().filter(|d| !d.is_instrument).map(|d| (d.id.clone(), label(&d.name, &d.vendor))).collect());
    (vst3, clap)
}

pub fn chain(app: &mut StudioApp, ui: &mut Ui) {
    let area = ui.max_rect();
    let p = ui.painter().clone();
    p.line_segment([area.left_top(), area.right_top()], Stroke::new(1.0, TOKENS.line));
    let mut layout = DevicesLayout { visible: true, area: r4(area), menu: std::mem::take(&mut app.mixer.devices.menu), ..DevicesLayout::default() };
    let Some(tid) = app.focus_track() else {
        p.text(area.center(), Align2::CENTER_CENTER, "Click a track to see its devices", FontId::proportional(12.0), TOKENS.text_dim);
        app.mixer.devices = layout;
        return;
    };
    let s = app.engine.session_arc();
    let Some(t) = s.track(tid) else { return };
    layout.track = Some(tid.0);
    // Track tab on the left: color and name.
    let tab = Rect::from_min_max(area.min, pos2(area.min.x + 120.0, area.max.y));
    p.rect_filled(tab, 0.0, TOKENS.panel_raised);
    p.rect_filled(Rect::from_min_size(tab.min, vec2(5.0, tab.height())), 0.0, theme::rgb(t.color));
    p.with_clip_rect(tab).text(pos2(tab.min.x + 12.0, tab.min.y + 16.0), Align2::LEFT_CENTER, &t.name, FontId::proportional(13.0), TOKENS.text);
    let used = t.mixer.inserts.iter().filter(|i| i.is_some()).count();
    let lane = Rect::from_min_max(pos2(tab.max.x + 8.0, area.min.y + 8.0), pos2(area.max.x - 8.0, area.max.y - 8.0));
    let mut x = lane.min.x;
    let mut cards: Vec<(Option<usize>, Insert)> = Vec::new();
    if t.kind == TrackKind::Instrument
        && let Some(i) = &t.instrument
    {
        cards.push((None, i.clone()));
    }
    for (slot, ins) in t.mixer.inserts.iter().enumerate() {
        if let Some(i) = ins {
            cards.push((Some(slot), i.clone()));
        }
    }
    for (slot, ins) in cards {
        let r = Rect::from_min_size(pos2(x, lane.min.y), vec2(CARD_W, lane.height()));
        if r.min.x < lane.max.x {
            let c = card(app, ui, &p, lane, tid, slot, &ins, r);
            layout.cards.push(c);
        }
        // An arrow between devices: the signal flows left to right.
        let ax = r.max.x + 7.0;
        p.with_clip_rect(lane).text(pos2(ax, r.center().y), Align2::CENTER_CENTER, "›", FontId::proportional(16.0), TOKENS.text_dim);
        x += CARD_W + 14.0;
    }
    // "+": add a device.
    let add = Rect::from_min_size(pos2(x, lane.min.y), vec2(40.0, lane.height()));
    layout.add = r4(add);
    let full = used >= t.mixer.inserts.len();
    let resp = ui.interact(add.intersect(lane), ui.id().with(("add_device", tid.0)), Sense::click());
    let pc = p.with_clip_rect(lane);
    pc.rect_filled(add, 4.0, if resp.hovered() && !full { TOKENS.button_hover } else { TOKENS.button });
    pc.text(add.center(), Align2::CENTER_CENTER, "+", FontId::proportional(20.0), if full { TOKENS.text_dim } else { TOKENS.text });
    let resp = resp.on_hover_text(if full { "All ten device slots are in use" } else { "Add an effect" });
    if !full {
        let builtin: Vec<PluginEntry> = app.builtin_plugins().into_iter().filter(|p| !p.instrument).collect();
        let mut picked: Option<String> = None;
        let menu = &mut layout.menu;
        egui::Popup::menu(&resp).width(220.0).show(|ui| {
            ui.label(egui::RichText::new("Built-in").color(TOKENS.text_dim).size(10.5));
            for e in &builtin {
                let b = ui.add(egui::Button::new(e.name.as_str()).right_text(egui::RichText::new(&e.category).color(TOKENS.text_dim).size(10.0)));
                menu.push(MenuItem { label: e.name.clone(), value: e.id.clone(), rect: r4(b.rect) });
                if b.clicked() {
                    picked = Some(e.id.clone());
                    ui.close();
                }
            }
            let (vst3, clap) = hosted_effects();
            for (format, list) in [("VST3", vst3), ("CLAP", clap)] {
                ui.separator();
                ui.menu_button(format, |ui| match list {
                    None => {
                        ui.weak("Not scanned yet (the scan runs at launch)");
                    }
                    Some(l) if l.is_empty() => {
                        ui.weak(format!("No {format} effects found"));
                    }
                    Some(l) => {
                        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                            for (id, label) in l {
                                if ui.button(&label).clicked() {
                                    picked = Some(id.clone());
                                    ui.close();
                                }
                            }
                        });
                    }
                });
            }
        });
        if let Some(id) = picked {
            let _ = app.add_device(tid, &id);
        }
    }
    app.mixer.devices = layout;
}

/// One device card: bypass (power), name, remove; kind; "Open editor".
#[allow(clippy::too_many_arguments)]
fn card(app: &mut StudioApp, ui: &mut Ui, p: &egui::Painter, lane: Rect, t: TrackId, slot: Option<usize>, ins: &Insert, r: Rect) -> DeviceCard {
    let pc = p.with_clip_rect(lane);
    let on = !ins.bypass;
    pc.rect_filled(r, 4.0, TOKENS.panel_raised);
    let title = Rect::from_min_size(r.min, vec2(r.width(), 22.0));
    pc.rect_filled(title, egui::CornerRadius { nw: 4, ne: 4, sw: 0, se: 0 }, if on { TOKENS.button } else { TOKENS.bg });
    let name = app.plugin_name(&ins.plugin);
    let mut c = DeviceCard { slot, plugin: ins.plugin.clone(), name: name.clone(), bypass: ins.bypass, rect: r4(r), ..DeviceCard::default() };
    let salt = slot.map_or(u64::MAX, |s| s as u64);
    // Power button (bypass) on inserts; the instrument has none (no instrument bypass command).
    let mut name_x = r.min.x + 8.0;
    if let Some(slot) = slot {
        let pb = Rect::from_min_size(pos2(r.min.x + 4.0, r.min.y + 3.0), vec2(16.0, 16.0));
        let resp = ui.interact(pb.intersect(lane), ui.id().with(("bypass", t.0, salt)), Sense::click());
        pc.circle_filled(pb.center(), 6.0, if on { TOKENS.accent } else { TOKENS.button_hover });
        if resp.on_hover_text(if on { "Bypass" } else { "Turn on" }).clicked() {
            let _ = app.toggle_bypass(t, slot);
        }
        c.bypass_button = r4(pb);
        name_x = pb.max.x + 5.0;
        let xb = Rect::from_min_size(pos2(r.max.x - 20.0, r.min.y + 3.0), vec2(16.0, 16.0));
        let resp = ui.interact(xb.intersect(lane), ui.id().with(("remove", t.0, salt)), Sense::click());
        pc.text(xb.center(), Align2::CENTER_CENTER, "×", FontId::proportional(14.0), if resp.hovered() { TOKENS.record } else { TOKENS.text_dim });
        if resp.on_hover_text("Remove").clicked() {
            let _ = app.remove_device(t, slot);
        }
        c.remove_button = r4(xb);
    }
    let name_r = Rect::from_min_max(pos2(name_x, title.min.y), pos2(r.max.x - 22.0, title.max.y));
    pc.with_clip_rect(name_r.intersect(lane)).text(
        name_r.left_center(),
        Align2::LEFT_CENTER,
        &name,
        FontId::proportional(13.0),
        if on { TOKENS.text } else { TOKENS.text_dim },
    );
    let kind = match (slot, ins.plugin.split(':').next()) {
        (None, _) => "Instrument".to_string(),
        (_, Some("vst3")) => "VST3 effect".into(),
        (_, Some("clap")) => "CLAP effect".into(),
        (_, Some("au")) => "Audio Unit".into(),
        _ => app.builtin_plugins().iter().find(|p| p.id == ins.plugin).map_or("Built-in".into(), |p| format!("Built-in · {}", p.category)),
    };
    pc.text(pos2(r.min.x + 8.0, title.max.y + 12.0), Align2::LEFT_CENTER, kind, FontId::proportional(10.0), TOKENS.text_dim);
    if ins.bypass {
        pc.text(pos2(r.min.x + 8.0, title.max.y + 28.0), Align2::LEFT_CENTER, "Bypassed", FontId::proportional(10.0), TOKENS.mute);
    }
    // Open editor: the plug-in's own window, through the same path as `ui.set {plugin_windows}`.
    let key_slot = slot.unwrap_or(soundcraft_mix::INSTRUMENT_SLOT);
    let enabled = app.player.as_ref().is_some_and(|pl| hosted(&ins.plugin) || pl.has_editor(t, key_slot));
    let open = app.ui.plugin_windows.contains(&(t, key_slot));
    let eb = Rect::from_min_size(pos2(r.min.x + 8.0, r.max.y - 26.0), vec2(r.width() - 16.0, 20.0));
    let resp = ui.interact(eb.intersect(lane), ui.id().with(("editor", t.0, salt)), Sense::click());
    pc.rect_filled(eb, 3.0, if resp.hovered() && enabled { TOKENS.button_hover } else { TOKENS.button });
    let label = if open { "Editor open" } else { "Open editor" };
    pc.text(
        eb.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(10.5),
        if enabled { TOKENS.text } else { TOKENS.text_dim.gamma_multiply(0.6) },
    );
    let tip = if enabled { "Open the plug-in's window" } else { "No editor window (built-in, or no audio device)" };
    if resp.on_hover_text(tip).clicked() && enabled {
        app.want_editor((t, key_slot));
    }
    c.editor_button = r4(eb);
    c.editor_enabled = enabled;
    c
}
