//! The mixer: one channel strip per track plus the master, as a pane under the arrangement
//! (Ableton's Ctrl+Alt+M) or filling the window (Cubase's F3). A strip has the track's name and
//! color, input and output dropdowns, a balance knob, Mute/Solo/Record/Monitor, a
//! fader and a meter. Flat, like the arrangement: fills, no borders or gradients, one accent.
//!
//! Everything a strip changes goes through engine commands. A fader or knob drag is one undo step
//! (`Engine::execute_merged` with a per-track key). Which strip is focused, the panes and the
//! scroll are view state in [`crate::UiState`] and [`MixerState`], never in the document.
//!
//! Model leaks handled here (plan 06, section 2):
//! - L10: stereo tracks pan dual-mono. The knob is a balance control that drives both panners
//!   through [`pans_for`] (two `mix.pan` calls under one merge key).
//! - L5: busses are named paths. Outputs name the FX/group channel (the aux track
//!   whose input is the bus), not the bus. "Add FX Channel" is four commands, one undo step.
//! - L12: the master is an optional track. Without one the main-out strip shows the main meter
//!   and offers to add a master fader.
//! - L4: sends are ten fixed slots. A strip lists the occupied ones as rows (target, level,
//!   remove) under its input, Logic style, and "+ Send" fills the first free slot.
//!
//! Routing never offers a target that would close a loop of any length: the engine accepts
//! loops (`track.output`, `track.input` and `mix.send` don't check), and the mix only bounds the
//! aux depth, so the menus walk the graph themselves ([`feeding_loops`], [`listening_loops`]).

use crate::StudioApp;
use crate::arrange::R4;
use crate::theme::{self, TOKENS};
use egui::{Align2, Color32, CursorIcon, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_model::{BusId, Route, SendSlot, Session, Track, TrackId, TrackKind};

/// Strip width and gap (points).
pub const STRIP_W: f32 = 88.0;
const GAP: f32 = 2.0;
/// The fader's top, in dB.
pub const FADER_MAX_DB: f32 = 12.0;

/// A drag in progress on a strip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MixGesture {
    Fader { track: TrackId, start_k: f32, height: f32 },
    Knob { track: TrackId, start: f32 },
    Send { track: TrackId, slot: usize, start_k: f32 },
}

/// The value a "+ Send" menu entry carries for "New FX Channel…".
pub const NEW_FX: &str = "\u{1}new-fx-channel";

/// One send row as last drawn (a strip's or the device chain's).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SendRow {
    pub slot: usize,
    /// What the target reads as: the FX channel's name, or "Bus: name".
    pub target: String,
    pub level_db: f32,
    pub pre_fader: bool,
    pub rect: R4,
    /// Click: a menu to send somewhere else.
    pub target_rect: R4,
    /// Drag: the level (one undo step); double-click: 0 dB.
    pub level_rect: R4,
    pub remove: R4,
    /// The pre/post toggle (device chain only; strips have it in the row's right-click menu).
    pub pre: R4,
}

/// One dropdown or menu entry as last drawn (for `ui.inspect`; scripts click them).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct MenuItem {
    pub label: String,
    pub value: String,
    pub rect: R4,
}

/// One strip as last drawn, with the values it shows.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct StripLayout {
    pub track: u64,
    pub name: String,
    pub kind: Option<TrackKind>,
    pub rect: R4,
    pub name_rect: R4,
    pub input: R4,
    pub output: R4,
    pub input_label: String,
    pub output_label: String,
    pub knob: R4,
    pub mute: R4,
    pub solo: R4,
    pub arm: R4,
    pub monitor: R4,
    pub fader: R4,
    pub meter: R4,
    pub volume_db: f32,
    /// The knob's value, -1 (left) .. 1 (right), read back from the track's panners.
    pub balance: f32,
    /// The track's panners as the engine holds them.
    pub pan: Vec<f32>,
    pub level: [f32; 2],
    /// An FX (aux) channel: tagged "FX", its name muted.
    pub fx: bool,
    pub sends: Vec<SendRow>,
    /// "+ Send" (zero when the strip takes no sends or all ten are in use).
    pub add_send: R4,
}

/// Last frame's mixer geometry.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct MixerLayout {
    pub visible: bool,
    pub area: R4,
    pub strips: Vec<StripLayout>,
    /// The master strip (`track` 0 when there is no master track yet).
    pub master: Option<StripLayout>,
    pub add_master: R4,
    pub add_fx: R4,
    /// The open dropdown's entries.
    pub menu: Vec<MenuItem>,
}

/// One device card as last drawn.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DeviceCard {
    /// Insert slot, or `None` for the instrument.
    pub slot: Option<usize>,
    pub plugin: String,
    pub name: String,
    pub bypass: bool,
    pub rect: R4,
    pub bypass_button: R4,
    pub remove_button: R4,
    pub editor_button: R4,
    pub editor_enabled: bool,
    /// The instrument card's name (or "Choose…" when there is none): opens the instrument menu.
    pub pick_button: R4,
}

/// Last frame's device chain geometry.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DevicesLayout {
    pub visible: bool,
    pub track: Option<u64>,
    pub area: R4,
    pub cards: Vec<DeviceCard>,
    pub add: R4,
    /// The open menu's entries: "+" or the instrument menu (`value` is the plug-in id), or a
    /// send menu (`value` is the bus name, or [`NEW_FX`]).
    pub menu: Vec<MenuItem>,
    /// The focused track's sends.
    pub sends: Vec<SendRow>,
    pub add_send: R4,
}

/// A plug-in the "+" menu offers.
#[derive(Debug, Clone)]
pub struct PluginEntry {
    pub id: String,
    pub name: String,
    pub category: String,
    pub instrument: bool,
}

#[derive(Debug, Default)]
pub struct MixerState {
    pub gesture: Option<MixGesture>,
    /// Horizontal scroll of the strips (points).
    pub hscroll: f32,
    pub layout: MixerLayout,
    pub devices: DevicesLayout,
    /// Built-in plug-ins (from `engine.plugins`), read once.
    pub(crate) builtin: Option<Vec<PluginEntry>>,
    /// Display names of hosted plug-ins by id, filled from the scan caches as cards need them.
    pub(crate) names: std::collections::HashMap<String, String>,
    /// The instrument menu's filter text (shown when the list is long).
    pub instrument_filter: String,
    /// Gestures started, to give each composite its own merge key.
    serial: u64,
}

/// Fader position 0..1 for a gain in dB: the fourth root of the amplitude, scaled so +12 dB is
/// the top. 0 dB sits at about 70 %, -60 dB at about 13 %; -144 dB (silence) is the bottom.
pub fn db_to_k(db: f32) -> f32 {
    if db <= -144.0 {
        return 0.0;
    }
    let top = 10f32.powf(FADER_MAX_DB / 80.0);
    (10f32.powf(db / 80.0) / top).clamp(0.0, 1.0)
}

/// The inverse of [`db_to_k`].
pub fn k_to_db(k: f32) -> f32 {
    if k <= 0.0005 {
        return -144.0;
    }
    let top = 10f32.powf(FADER_MAX_DB / 80.0);
    (80.0 * (k.min(1.0) * top).log10()).clamp(-144.0, FADER_MAX_DB)
}

/// The panners for balance `b` (-1..1) on a track with `n` of them (plan 06, L10). Mono: the one
/// panner is `b`. Stereo: center keeps L hard left and R hard right; turning toward a side moves
/// the other channel's panner across, so at the end both sit on that side.
pub fn pans_for(b: f32, n: usize) -> Vec<f32> {
    let b = b.clamp(-1.0, 1.0);
    match n {
        0 => Vec::new(),
        1 => vec![b],
        _ => vec![-1.0 + 2.0 * b.max(0.0), 1.0 + 2.0 * b.min(0.0)],
    }
}

/// The balance a set of panners stands for (the inverse of [`pans_for`]).
pub fn balance_of(pan: &[f32]) -> f32 {
    match pan {
        [] => 0.0,
        [p] => *p,
        [l, r, ..] => ((l + r) / 2.0).clamp(-1.0, 1.0),
    }
}

fn r4(r: Rect) -> R4 {
    [r.min.x, r.min.y, r.max.x, r.max.y]
}

fn meter_k(lin: f32) -> f32 {
    if lin <= 0.0 {
        return 0.0;
    }
    ((20.0 * lin.log10() + 60.0) / 66.0).clamp(0.0, 1.0)
}

/// The aux track fed by `bus`, if any: an FX or group channel.
fn channel_of_bus(s: &Session, bus: BusId) -> Option<&Track> {
    s.tracks.iter().find(|t| t.kind == TrackKind::Aux && t.mixer.input == Route::Bus(bus))
}

/// A bus as a routing target reads: the FX channel it feeds, else "Bus: name".
pub fn bus_label(s: &Session, bus: BusId) -> String {
    channel_of_bus(s, bus).map_or_else(|| format!("Bus: {}", s.bus(bus).map_or("?", |b| b.name.as_str())), |c| c.name.clone())
}

/// The busses a track feeds: its output and its sends' targets.
fn bus_targets(t: &Track) -> impl Iterator<Item = BusId> + '_ {
    std::iter::once(&t.mixer.output).chain(t.mixer.sends.iter().flatten().map(|s| &s.target)).filter_map(|r| match r {
        Route::Bus(b) => Some(*b),
        _ => None,
    })
}

/// Whether audio put on bus `from` reaches bus `to`: through every track listening to a bus and
/// on through its output and sends, any number of hops (`from == to` counts).
pub fn reaches(s: &Session, from: BusId, to: BusId) -> bool {
    let mut seen: Vec<BusId> = Vec::new();
    let mut todo = vec![from];
    while let Some(b) = todo.pop() {
        if b == to {
            return true;
        }
        if seen.contains(&b) {
            continue;
        }
        seen.push(b);
        for t in s.tracks.iter().filter(|t| t.mixer.input == Route::Bus(b)) {
            todo.extend(bus_targets(t));
        }
    }
    false
}

/// Whether `t` feeding bus `b` (output or send) would close a loop back to `t`'s own input.
pub fn feeding_loops(s: &Session, t: &Track, b: BusId) -> bool {
    matches!(t.mixer.input, Route::Bus(input) if reaches(s, b, input))
}

/// Whether `t` listening to bus `b` would close a loop: something `t` feeds reaches `b`.
pub fn listening_loops(s: &Session, t: &Track, b: BusId) -> bool {
    bus_targets(t).any(|o| reaches(s, o, b))
}

/// Whether a track kind has sends here: audio, instrument and FX channels.
pub fn takes_sends(kind: TrackKind) -> bool {
    matches!(kind, TrackKind::Audio | TrackKind::Instrument | TrackKind::Aux)
}

/// Where a send from `t` can go: `(label, bus name)` for every bus that is not already one of its
/// sends (`except` slot aside) and would not close a loop.
pub fn send_targets(s: &Session, t: &Track, except: Option<usize>) -> Vec<(String, String)> {
    let taken: Vec<&Route> =
        t.mixer.sends.iter().enumerate().filter(|(i, _)| Some(*i) != except).filter_map(|(_, x)| x.as_ref()).map(|x| &x.target).collect();
    s.busses
        .iter()
        .filter(|b| !taken.contains(&&Route::Bus(b.id)) && !feeding_loops(s, t, b.id))
        .map(|b| (bus_label(s, b.id), b.name.clone()))
        .collect()
}

/// Whether picking `value` from a route dropdown would leave the route as it is: the same route,
/// or the label the current one already reads as (an audio track's default input, L15).
fn same_route(s: &Session, cur: &Route, cur_label: &str, value: &str, label: &str) -> bool {
    if label == cur_label {
        return true;
    }
    let parsed = match value.to_ascii_lowercase().as_str() {
        "" | "none" | "no input" | "no output" => Route::None,
        "main" | "out 1-2" | "master" => Route::Main,
        _ => s.bus_by_name(value).map_or_else(|| Route::Hardware(value.to_string()), |b| Route::Bus(b.id)),
    };
    &parsed == cur
}

/// How a route reads in a dropdown: busses by the channel they feed (L5). An audio track whose
/// input is `None` or `Main` records and monitors from input 1 (`engine::io::plan_takes`, the mix
/// engine's monitoring), so it reads as that input, not "No input".
pub fn route_label(s: &Session, r: &Route, input: bool, format_channels: usize) -> String {
    match r {
        Route::None | Route::Main if input && format_channels > 0 => if format_channels >= 2 { "In 1-2" } else { "In 1" }.into(),
        Route::None => if input { "No input" } else { "No output" }.into(),
        Route::Main => "Main".into(),
        Route::Bus(b) if input => s.bus(*b).map_or_else(|| "Bus".into(), |b| format!("Bus: {}", b.name)),
        Route::Bus(b) => bus_label(s, *b),
        Route::Hardware(h) => h.clone(),
    }
}

/// The outputs a track can pick: `(label, value for track.output)`.
pub fn output_choices(s: &Session, t: &Track) -> Vec<(String, String)> {
    let mut v = vec![("Main".to_string(), "main".to_string())];
    for b in &s.busses {
        // Never a bus that comes back to this track's input, however many hops away.
        if feeding_loops(s, t, b.id) {
            continue;
        }
        v.push((bus_label(s, b.id), b.name.clone()));
    }
    v.push(("No output".into(), "none".into()));
    v
}

/// The inputs a track can pick: hardware inputs (`In 1`.. or `In 1-2`..) and busses.
pub fn input_choices(s: &Session, t: &Track, device_inputs: usize) -> Vec<(String, String)> {
    let mut v = Vec::new();
    let n = device_inputs.max(2);
    if t.kind == TrackKind::Audio || t.kind == TrackKind::Aux {
        if t.channels() >= 2 {
            for i in (1..n).step_by(2) {
                v.push((format!("In {i}-{}", i + 1), format!("In {i}-{}", i + 1)));
            }
        } else {
            for i in 1..=n {
                v.push((format!("In {i}"), format!("In {i}")));
            }
        }
    }
    for b in &s.busses {
        if listening_loops(s, t, b.id) {
            continue;
        }
        v.push((format!("Bus: {}", b.name), b.name.clone()));
    }
    // An audio track with no input still records from input 1 (see `route_label`), so only an
    // FX or group channel is offered "No input".
    if t.kind == TrackKind::Aux {
        v.push(("No input".into(), "none".into()));
    }
    v
}

impl StudioApp {
    /// The focused track (device chain, highlight): the last clicked one, else the cursor's
    /// track, else the first selected track.
    pub fn focus_track(&self) -> Option<TrackId> {
        let s = self.engine.session();
        [self.ui.focus_track, self.ui.cursor_track, s.edit.selected_tracks.first().copied()].into_iter().flatten().find(|t| s.track(*t).is_some())
    }

    /// Focus a track from either view: it becomes the selected track (as a click on its header
    /// does) and the cursor's track. Selection is not part of the saved document.
    pub fn focus(&mut self, t: TrackId) {
        self.ui.focus_track = Some(t);
        if self.engine.session().edit.selected_tracks != [t] {
            let _ = self.run("edit.select", json!({"tracks": [t.0]}));
        }
        let at = self.cursor();
        self.set_cursor(at, Some(t));
    }

    /// Fader gesture: one undo step per drag (merge key `fader:<track>`).
    pub fn set_volume_merged(&mut self, t: TrackId, db: f32) -> Result<Value, String> {
        self.engine.execute_merged("mix.volume", &json!({"tracks": [t.0], "db": db}), &format!("fader:{}", t.0)).map_err(|e| e.to_string())
    }

    /// Balance knob (L10): both panners of a stereo track, one undo step per gesture.
    pub fn set_balance(&mut self, t: TrackId, b: f32) -> Result<Value, String> {
        let n = self.engine.session().track(t).map_or(0, |tr| tr.mixer.pan.len());
        let key = format!("pan:{}", t.0);
        let mut last = json!({});
        for (i, p) in pans_for(b, n).into_iter().enumerate() {
            last = self.engine.execute_merged("mix.pan", &json!({"tracks": [t.0], "pan": p, "index": i}), &key).map_err(|e| e.to_string())?;
        }
        Ok(last)
    }

    /// Route a track's output (`track.output`), by the value an output choice carries. Picking
    /// what the output already is changes nothing (no document edit, no undo step).
    pub fn set_output(&mut self, t: TrackId, value: &str) -> Result<Value, String> {
        let s = self.engine.session_arc();
        if let Some(tr) = s.track(t) {
            let cur = route_label(&s, &tr.mixer.output, false, tr.channels());
            let label = output_choices(&s, tr).into_iter().find(|(_, v)| v == value).map_or_else(|| value.to_string(), |(l, _)| l);
            if same_route(&s, &tr.mixer.output, &cur, value, &label) {
                return Ok(json!({"unchanged": true}));
            }
        }
        self.run("track.output", json!({"tracks": [t.0], "output": value}))
    }

    /// Route a track's input (`track.input`). Picking what the input already reads as (an audio
    /// track's default "In 1-2", L15) changes nothing.
    pub fn set_input(&mut self, t: TrackId, value: &str) -> Result<Value, String> {
        let s = self.engine.session_arc();
        if let Some(tr) = s.track(t) {
            let ch = if tr.kind == TrackKind::Audio { tr.channels() } else { 0 };
            let cur = route_label(&s, &tr.mixer.input, true, ch);
            let label =
                input_choices(&s, tr, self.device_inputs()).into_iter().find(|(_, v)| v == value).map_or_else(|| value.to_string(), |(l, _)| l);
            if same_route(&s, &tr.mixer.input, &cur, value, &label) {
                return Ok(json!({"unchanged": true}));
            }
        }
        self.run("track.input", json!({"tracks": [t.0], "input": value}))
    }

    /// Add a device at the end of the track's chain: the slot after the last occupied one, so it
    /// lands after every device already there. Ten slots at most (plan 06, L1): when the last
    /// slot is taken but an earlier one is free, the device goes there and the status line says so.
    pub fn add_device(&mut self, t: TrackId, plugin: &str) -> Result<Value, String> {
        let ins = self.engine.session().track(t).map(|tr| tr.mixer.inserts.clone()).unwrap_or_default();
        let next = ins.iter().rposition(Option::is_some).map_or(0, |i| i + 1);
        if next < ins.len() {
            return self.run("mix.insert", json!({"track": t.0, "slot": next, "plugin": plugin}));
        }
        let r = self.run("mix.insert", json!({"track": t.0, "plugin": plugin}));
        if r.is_ok() {
            self.status =
                "The last device slot is in use, so the new device went into a free slot earlier in the chain (ten slots, plan 06 L1).".into();
        }
        r
    }

    pub fn toggle_bypass(&mut self, t: TrackId, slot: usize) -> Result<Value, String> {
        self.run("mix.insert_bypass", json!({"track": t.0, "slot": slot}))
    }

    /// Remove a device; its editor window closes first.
    pub fn remove_device(&mut self, t: TrackId, slot: usize) -> Result<Value, String> {
        if let Some(p) = &self.player {
            p.close_editor(t, slot);
        }
        self.ui.plugin_windows.retain(|k| *k != (t, slot));
        self.run("mix.insert_remove", json!({"track": t.0, "slot": slot}))
    }

    /// Put `plugin` on an instrument track as its instrument (`mix.instrument`, one undo step),
    /// replacing the one there. The old instrument's editor window closes first. Picking the
    /// instrument the track already has changes nothing.
    pub fn set_instrument(&mut self, t: TrackId, plugin: &str) -> Result<Value, String> {
        if self.engine.session().track(t).and_then(|tr| tr.instrument.as_ref()).is_some_and(|i| i.plugin == plugin) {
            return Ok(json!({"unchanged": true}));
        }
        let key = (t, soundcraft_mix::INSTRUMENT_SLOT);
        if let Some(p) = &self.player {
            p.close_editor(key.0, key.1);
        }
        self.ui.plugin_windows.retain(|k| *k != key);
        self.editors.retain(|e| e.key != key);
        self.run("mix.instrument", json!({"track": t.0, "plugin": plugin}))
    }

    /// "+ FX Channel" after the strips: a new FX channel with a send from the focused track
    /// (not from an FX channel or the master) at -12 dB.
    pub fn add_fx_channel(&mut self) -> Result<TrackId, String> {
        let s = self.engine.session();
        let from = self.focus_track().filter(|t| s.track(*t).is_some_and(|t| t.kind != TrackKind::Aux && takes_sends(t.kind)));
        self.new_fx_channel(from, -12.0)
    }

    /// New FX Channel (plan 06, L5): a bus, an FX (aux) channel listening to it and, from `from`,
    /// a send to it at `level_db`, as one undo step. Returns the new channel's track id.
    ///
    /// When `from` has no free send slot nothing is made. If a step fails anyway, the steps
    /// already run are undone, so no bus or channel is left behind. The undo step reads "New Bus"
    /// (its first command's label): the engine has no way to name a composite (plan 06, L20).
    pub fn new_fx_channel(&mut self, from: Option<TrackId>, level_db: f32) -> Result<TrackId, String> {
        let s = self.engine.session_arc();
        let name =
            (1..).map(|i| format!("FX {i}")).find(|n| s.bus_by_name(n).is_none() && s.track_by_name(n).is_none()).unwrap_or_else(|| "FX".into());
        let slot = from.and_then(|f| s.track(f)).map(|t| (t.name.clone(), t.mixer.sends.iter().position(Option::is_none)));
        if let Some((track, None)) = &slot {
            self.status = format!("New FX Channel: all ten sends of {track} are in use");
            return Err(self.status.clone());
        }
        let slot = slot.and_then(|(_, s)| s);
        self.mixer.serial += 1;
        let key = format!("fx-channel:{}", self.mixer.serial);
        self.engine.end_merge();
        let r = (|| {
            let e = &mut self.engine;
            e.execute_merged("mix.new_bus", &json!({"name": name}), &key)?;
            let r = e.execute_merged("track.new", &json!({"kind": "aux", "format": "stereo", "name": name}), &key)?;
            let id = r.get("tracks").and_then(|t| t.get(0)).and_then(Value::as_u64).unwrap_or(0);
            e.execute_merged("track.input", &json!({"tracks": [id], "input": name}), &key)?;
            if let Some(f) = from {
                let mut p = json!({"track": f.0, "bus": name, "level_db": level_db});
                if let Some(slot) = slot {
                    p["slot"] = json!(slot);
                }
                e.execute_merged("mix.send", &p, &key)?;
            }
            Ok::<u64, soundcraft_engine::EngineError>(id)
        })();
        self.engine.end_merge();
        match r {
            Ok(id) => {
                self.status = format!("Added {name}{}", from.map_or(String::new(), |_| " with a send to it".into()));
                Ok(TrackId(id))
            }
            Err(e) => {
                // Take back what the composite did so far (one merged undo step).
                if !std::sync::Arc::ptr_eq(&s, &self.engine.session_arc()) {
                    self.engine.undo();
                }
                self.status = format!("New FX Channel: {e}");
                Err(self.status.clone())
            }
        }
    }

    /// "+ Send": a send from `t` to bus `value` at 0 dB in the first free slot, or a new FX
    /// channel with its send for [`NEW_FX`].
    pub fn add_send(&mut self, t: TrackId, value: &str) -> Result<Value, String> {
        if value == NEW_FX {
            return self.new_fx_channel(Some(t), 0.0).map(|id| json!({"track": id.0}));
        }
        self.run("mix.send", json!({"track": t.0, "bus": value, "level_db": 0.0}))
    }

    /// Send slot `slot` somewhere else; its level and pre/post setting stay.
    pub fn retarget_send(&mut self, t: TrackId, slot: usize, bus: &str) -> Result<Value, String> {
        self.run("mix.send", json!({"track": t.0, "slot": slot, "bus": bus}))
    }

    /// Send level gesture: one undo step per drag (merge key `send:<track>:<slot>`).
    pub fn set_send_level_merged(&mut self, t: TrackId, slot: usize, db: f32) -> Result<Value, String> {
        self.engine
            .execute_merged("mix.send_level", &json!({"track": t.0, "slot": slot, "db": db}), &format!("send:{}:{slot}", t.0))
            .map_err(|e| e.to_string())
    }

    pub fn remove_send(&mut self, t: TrackId, slot: usize) -> Result<Value, String> {
        self.run("mix.send_remove", json!({"track": t.0, "slot": slot}))
    }

    /// Pre-fader or post-fader (`mix.send_level {pre_fader}`).
    pub fn set_send_pre(&mut self, t: TrackId, slot: usize, pre: bool) -> Result<Value, String> {
        self.run("mix.send_level", json!({"track": t.0, "slot": slot, "pre_fader": pre}))
    }

    /// The number of audio inputs offered in input dropdowns: the open input's, else 8.
    fn device_inputs(&self) -> usize {
        self.recorder.as_ref().map_or(8, |r| r.channels)
    }
}

/// The bottom panes: device chain (lowest) and the mixer above it. Call before the central panel.
pub fn panes(app: &mut StudioApp, ui: &mut Ui) {
    app.mixer.layout.menu.clear();
    app.mixer.devices.menu.clear();
    app.mixer.devices.visible = app.ui.show_devices;
    if app.ui.show_devices {
        egui::Panel::bottom("devices")
            .exact_size(crate::devices::PANE_H)
            .frame(egui::Frame::NONE.fill(TOKENS.panel))
            .show(ui, |ui| crate::devices::chain(app, ui));
    }
    app.mixer.layout.visible = app.ui.show_mixer || app.ui.mixer_full;
    if app.ui.show_mixer && !app.ui.mixer_full {
        let h = app.ui.mixer_height.clamp(260.0, 600.0);
        egui::Panel::bottom("mixer").exact_size(h).frame(egui::Frame::NONE.fill(TOKENS.bg)).show(ui, |ui| strips(app, ui));
    }
}

/// The mixer filling the window (F3).
pub fn show_full(app: &mut StudioApp, ui: &mut Ui) {
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(TOKENS.bg)).show(ui, |ui| strips(app, ui));
}

fn strips(app: &mut StudioApp, ui: &mut Ui) {
    let area = ui.max_rect();
    let p = ui.painter();
    p.line_segment([area.left_top(), area.right_top()], Stroke::new(1.0, TOKENS.line));
    let s = app.engine.session_arc();
    let focus = app.focus_track();
    let master = s.tracks.iter().find(|t| t.kind == TrackKind::Master && !t.hidden);
    let channels: Vec<&Track> = s.tracks.iter().filter(|t| t.kind != TrackKind::Master && !t.hidden).collect();
    // The master strip is pinned right; the others scroll under it.
    let master_rect = Rect::from_min_max(pos2(area.max.x - STRIP_W - 6.0, area.min.y + 4.0), pos2(area.max.x - 6.0, area.max.y - 4.0));
    let lane = Rect::from_min_max(pos2(area.min.x + 4.0, area.min.y + 4.0), pos2(master_rect.min.x - 10.0, area.max.y - 4.0));
    let content_w = channels.len() as f32 * (STRIP_W + GAP) + 110.0;
    app.mixer.hscroll = app.mixer.hscroll.clamp(0.0, (content_w - lane.width()).max(0.0));
    let mut layout = MixerLayout { visible: true, area: r4(area), menu: std::mem::take(&mut app.mixer.layout.menu), ..MixerLayout::default() };
    // The sends block is as tall on every strip (the most sends plus "+ Send"), so the faders
    // line up, and never so tall that a fader gets shorter than MIN_FADER.
    let wanted = channels.iter().filter(|t| takes_sends(t.kind)).map(|t| send_rows_of(t)).max().unwrap_or(0);
    let room = ((lane.height() - STRIP_FIXED_H - MIN_FADER) / SEND_ROW_H).floor().max(0.0) as usize;
    let rows = wanted.min(room);
    for (i, t) in channels.iter().enumerate() {
        let x = lane.min.x + i as f32 * (STRIP_W + GAP) - app.mixer.hscroll;
        let r = Rect::from_min_size(pos2(x, lane.min.y), vec2(STRIP_W, lane.height()));
        if r.max.x < lane.min.x || r.min.x > lane.max.x {
            continue;
        }
        let sl = strip(app, ui, &s, t, r, lane, focus == Some(t.id), rows, &mut layout.menu);
        layout.strips.push(sl);
    }
    // "Add FX Channel" after the last strip.
    let ax = lane.min.x + channels.len() as f32 * (STRIP_W + GAP) - app.mixer.hscroll + 6.0;
    let add = Rect::from_min_size(pos2(ax, lane.min.y + 4.0), vec2(96.0, 24.0));
    if add.min.x < lane.max.x {
        let resp = ui.interact(add.intersect(lane), ui.id().with("add_fx"), Sense::click());
        let p = ui.painter().with_clip_rect(lane);
        p.rect_filled(add, 3.0, if resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
        p.text(add.center(), Align2::CENTER_CENTER, "+ FX Channel", FontId::proportional(11.5), TOKENS.text);
        if resp.on_hover_text("A return channel with a send from the focused track").clicked() {
            let _ = app.add_fx_channel();
        }
    }
    layout.add_fx = r4(add);
    // Master.
    ui.painter().line_segment([pos2(master_rect.min.x - 5.0, area.min.y), pos2(master_rect.min.x - 5.0, area.max.y)], Stroke::new(1.0, TOKENS.line));
    match master {
        Some(m) => {
            let sl = strip(app, ui, &s, m, master_rect, master_rect, focus == Some(m.id), rows, &mut layout.menu);
            layout.master = Some(sl);
        }
        None => {
            let (sl, add) = main_out(app, ui, master_rect);
            layout.master = Some(sl);
            layout.add_master = add;
        }
    }
    // Wheel over the strips scrolls them sideways.
    if let Some(pos) = ui.input(|i| i.pointer.hover_pos())
        && lane.contains(pos)
        && app.mixer.gesture.is_none()
    {
        let d = ui.input(|i| i.smooth_scroll_delta);
        let d = if d.x.abs() > d.y.abs() { d.x } else { d.y };
        if d.abs() > 0.1 {
            app.mixer.hscroll = (app.mixer.hscroll - d).clamp(0.0, (content_w - lane.width()).max(0.0));
        }
    }
    app.mixer.layout = layout;
}

/// The main-out strip when the session has no master track: the main meter and an offer.
fn main_out(app: &mut StudioApp, ui: &mut Ui, r: Rect) -> (StripLayout, R4) {
    let p = ui.painter();
    p.rect_filled(r, 3.0, TOKENS.panel);
    p.text(pos2(r.center().x, r.min.y + 12.0), Align2::CENTER_CENTER, "Main", FontId::proportional(12.0), TOKENS.text);
    let add = Rect::from_min_size(pos2(r.min.x + 6.0, r.min.y + 28.0), vec2(r.width() - 12.0, 34.0));
    let resp = ui.interact(add, ui.id().with("add_master"), Sense::click());
    p.rect_filled(add, 3.0, if resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
    p.text(add.center(), Align2::CENTER_CENTER, "+ Master\nfader", FontId::proportional(10.5), TOKENS.text);
    if resp.clicked() {
        let _ = app.run("track.new", json!({"kind": "master"}));
    }
    let meter = Rect::from_min_max(pos2(r.center().x - 8.0, r.min.y + 76.0), pos2(r.center().x + 8.0, r.max.y - 26.0));
    draw_meter(p, meter, app.main_meter.level, app.main_meter.clip);
    let sl = StripLayout { name: "Main".into(), rect: r4(r), meter: r4(meter), level: app.main_meter.level, ..StripLayout::default() };
    (sl, r4(add))
}

fn draw_meter(p: &egui::Painter, r: Rect, level: [f32; 2], clip: bool) {
    p.rect_filled(r, 1.0, TOKENS.bg);
    let w = (r.width() - 3.0) / 2.0;
    for (c, lvl) in level.iter().enumerate() {
        let x = r.min.x + 1.0 + c as f32 * (w + 1.0);
        let h = (r.height() - 2.0) * meter_k(*lvl);
        let bar = Rect::from_min_max(pos2(x, r.max.y - 1.0 - h), pos2(x + w, r.max.y - 1.0));
        let col = if clip {
            TOKENS.record
        } else if *lvl > 0.9 {
            TOKENS.solo
        } else {
            TOKENS.monitor
        };
        p.rect_filled(bar, 0.0, col);
    }
}

/// A small text button inside a strip; returns its response.
fn small_button(ui: &mut Ui, clip: Rect, r: Rect, id: egui::Id, label: &str, on: bool, color: Color32, enabled: bool) -> egui::Response {
    let resp = ui.interact(r.intersect(clip), id, Sense::click());
    let p = ui.painter().with_clip_rect(clip);
    let fill = if on {
        color
    } else if resp.hovered() && enabled {
        TOKENS.button_hover
    } else {
        TOKENS.button
    };
    p.rect_filled(r, 2.0, fill);
    let fg = if on {
        theme::text_on(color)
    } else if enabled {
        TOKENS.text
    } else {
        TOKENS.text_dim.gamma_multiply(0.5)
    };
    p.text(r.center(), Align2::CENTER_CENTER, label, FontId::proportional(11.0), fg);
    resp
}

/// A dropdown field: its current value, and a menu of `choices` when clicked. Returns the value
/// picked. The open menu's entries go to `menu` for `ui.inspect`.
fn dropdown(ui: &mut Ui, clip: Rect, r: Rect, id: egui::Id, current: &str, choices: &[(String, String)], menu: &mut Vec<MenuItem>) -> Option<String> {
    let resp = ui.interact(r.intersect(clip), id, Sense::click());
    let p = ui.painter().with_clip_rect(clip);
    p.rect_filled(r, 2.0, if resp.hovered() { TOKENS.button_hover } else { TOKENS.bg });
    let text_r = r.shrink2(vec2(4.0, 0.0));
    p.with_clip_rect(text_r.intersect(clip)).text(text_r.left_center(), Align2::LEFT_CENTER, current, FontId::proportional(10.5), TOKENS.text);
    let c = pos2(r.max.x - 7.0, r.center().y);
    p.add(egui::Shape::convex_polygon(
        vec![pos2(c.x - 3.0, c.y - 1.5), pos2(c.x + 3.0, c.y - 1.5), pos2(c.x, c.y + 2.0)],
        TOKENS.text_dim,
        Stroke::NONE,
    ));
    choice_menu(&resp, current, choices, menu)
}

/// The menu of a dropdown-like field: `choices` as `(label, value)`, the `current` label marked.
/// A [`NEW_FX`] entry comes after a separator. Returns the value picked.
fn choice_menu(resp: &egui::Response, current: &str, choices: &[(String, String)], menu: &mut Vec<MenuItem>) -> Option<String> {
    let mut picked = None;
    egui::Popup::menu(resp).width(150.0).show(|ui| {
        if choices.is_empty() {
            ui.weak("Nothing to pick");
        }
        for (label, value) in choices {
            if value == NEW_FX {
                ui.separator();
            }
            let b = ui.add(egui::Button::selectable(label == current, label.as_str()));
            menu.push(MenuItem { label: label.clone(), value: value.clone(), rect: r4(b.rect) });
            if b.clicked() {
                picked = Some(value.clone());
                ui.close();
            }
        }
    });
    picked
}

/// Height of one send row, and of the strip without its sends block and fader.
pub const SEND_ROW_H: f32 = 17.0;
const STRIP_FIXED_H: f32 = 195.0;
const MIN_FADER: f32 = 80.0;

/// The rows a track's sends block wants: its sends plus "+ Send" while a slot is free.
fn send_rows_of(t: &Track) -> usize {
    let used = t.mixer.sends.iter().flatten().count();
    used + usize::from(used < t.mixer.sends.len())
}

/// How a send level reads: whole dB in a strip, tenths in the device chain.
fn level_text(db: f32, wide: bool) -> String {
    if db <= -144.0 {
        "-inf".into()
    } else if wide {
        format!("{db:.1} dB")
    } else if db.abs() < 0.5 {
        "0".into()
    } else {
        format!("{db:.0}")
    }
}

/// One send row: the target (click: send it somewhere else; right-click: pre/post and remove),
/// the level (drag like a fader, one undo step; double-click: 0 dB) and remove. `wide` (the
/// device chain) adds a Pre/Post toggle; a strip marks a pre-fader send with a "P".
#[allow(clippy::too_many_arguments)]
pub(crate) fn send_row(
    app: &mut StudioApp,
    ui: &mut Ui,
    s: &Session,
    t: &Track,
    slot: usize,
    snd: &SendSlot,
    r: Rect,
    clip: Rect,
    wide: bool,
    menu: &mut Vec<MenuItem>,
) -> SendRow {
    let id = t.id;
    let salt = (id.0, slot, wide);
    let p = ui.painter().with_clip_rect(clip);
    let target = match &snd.target {
        Route::Bus(b) => bus_label(s, *b),
        other => route_label(s, other, false, 0),
    };
    let mut row = SendRow { slot, target: target.clone(), level_db: snd.level_db, pre_fader: snd.pre_fader, rect: r4(r), ..SendRow::default() };
    p.rect_filled(r, 2.0, TOKENS.bg);
    // The level as a thin line along the bottom, so the row reads as a small fader.
    let k = db_to_k(snd.level_db);
    p.rect_filled(
        Rect::from_min_size(pos2(r.min.x, r.max.y - 2.0), vec2(r.width() * k, 2.0)),
        0.0,
        if snd.mute { TOKENS.text_dim } else { TOKENS.accent },
    );
    let xb = Rect::from_min_max(pos2(r.max.x - 14.0, r.min.y), r.max);
    let lr = Rect::from_min_max(pos2(xb.min.x - if wide { 50.0 } else { 24.0 }, r.min.y), pos2(xb.min.x, r.max.y));
    let pr = Rect::from_min_max(
        pos2(lr.min.x - if wide { 34.0 } else { 0.0 }, r.min.y + 2.0),
        pos2(lr.min.x - if wide { 4.0 } else { 0.0 }, r.max.y - 3.0),
    );
    let mut tr = Rect::from_min_max(r.min, pos2(pr.min.x - 1.0, r.max.y));
    if !wide && snd.pre_fader {
        p.text(pos2(tr.min.x + 7.0, tr.center().y), Align2::CENTER_CENTER, "P", FontId::proportional(9.5), TOKENS.text_dim);
        tr.min.x += 12.0;
    }
    // Target: another bus, or right-click for pre/post and remove.
    let resp = ui.interact(tr.intersect(clip), ui.id().with(("send_target", salt)), Sense::click());
    if resp.hovered() {
        p.rect_filled(tr, 2.0, TOKENS.button_hover);
    }
    let text_r = tr.shrink2(vec2(4.0, 0.0));
    p.with_clip_rect(text_r.intersect(clip)).text(text_r.left_center(), Align2::LEFT_CENTER, &target, FontId::proportional(10.5), TOKENS.text);
    row.target_rect = r4(tr);
    let mut choices = send_targets(s, t, Some(slot));
    if !choices.iter().any(|(l, _)| *l == target) {
        choices.insert(0, (target.clone(), String::new()));
    }
    if let Some(v) = choice_menu(&resp, &target, &choices, menu)
        && !v.is_empty()
        && v != target
    {
        let _ = app.retarget_send(id, slot, &v);
    }
    resp.context_menu(|ui| {
        let label = if snd.pre_fader { "Post-fader" } else { "Pre-fader" };
        if ui.button(label).clicked() {
            let _ = app.set_send_pre(id, slot, !snd.pre_fader);
            ui.close();
        }
        if ui.button("Remove Send").clicked() {
            let _ = app.remove_send(id, slot);
            ui.close();
        }
    });
    // Pre/Post (device chain).
    if wide {
        let resp = ui.interact(pr.intersect(clip), ui.id().with(("send_pre", salt)), Sense::click());
        p.rect_filled(pr, 2.0, if snd.pre_fader || resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
        p.text(
            pr.center(),
            Align2::CENTER_CENTER,
            if snd.pre_fader { "Pre" } else { "Post" },
            FontId::proportional(10.0),
            if snd.pre_fader { TOKENS.text } else { TOKENS.text_dim },
        );
        if resp.on_hover_text("Pre-fader or post-fader send").clicked() {
            let _ = app.set_send_pre(id, slot, !snd.pre_fader);
        }
        row.pre = r4(pr);
    }
    // Level: drag up or right to raise it, like the fader (fourth-root law).
    let resp = ui.interact(lr.intersect(clip), ui.id().with(("send_level", salt)), Sense::click_and_drag());
    let hot = resp.hovered() || matches!(app.mixer.gesture, Some(MixGesture::Send { track, slot: sl, .. }) if track == id && sl == slot);
    p.text(
        pos2(lr.max.x - 2.0, lr.center().y - 1.0),
        Align2::RIGHT_CENTER,
        level_text(snd.level_db, wide),
        FontId::monospace(10.0),
        if hot { TOKENS.accent } else { TOKENS.text },
    );
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    }
    if resp.drag_started() {
        app.mixer.gesture = Some(MixGesture::Send { track: id, slot, start_k: k });
    }
    if resp.dragged()
        && let Some(MixGesture::Send { track, slot: sl, start_k }) = app.mixer.gesture
        && track == id
        && sl == slot
    {
        let d = ui.input(|i| i.pointer.press_origin().zip(i.pointer.latest_pos()).map_or(0.0, |(a, b)| (b.x - a.x) - (b.y - a.y)));
        let fine = ui.input(|i| i.modifiers.shift);
        let nk = (start_k + d / 150.0 * if fine { 0.2 } else { 1.0 }).clamp(0.0, 1.0);
        let _ = app.set_send_level_merged(id, slot, k_to_db(nk));
    }
    if resp.drag_stopped() {
        app.mixer.gesture = None;
    }
    if resp.double_clicked() {
        let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "db": 0.0}));
    }
    let _ = resp.on_hover_text(format!(
        "{target}: {} ({}). Drag to change, double-click for 0 dB",
        level_text(snd.level_db, true),
        if snd.pre_fader { "pre-fader" } else { "post-fader" }
    ));
    row.level_rect = r4(lr);
    // Remove.
    let resp = ui.interact(xb.intersect(clip), ui.id().with(("send_remove", salt)), Sense::click());
    p.text(xb.center(), Align2::CENTER_CENTER, "×", FontId::proportional(12.0), if resp.hovered() { TOKENS.record } else { TOKENS.text_dim });
    if resp.on_hover_text("Remove send").clicked() {
        let _ = app.remove_send(id, slot);
    }
    row.remove = r4(xb);
    row
}

/// "+ Send": the busses a send from `t` can go to, then "New FX Channel…". Returns its rect.
pub(crate) fn add_send_row(app: &mut StudioApp, ui: &mut Ui, s: &Session, t: &Track, r: Rect, clip: Rect, menu: &mut Vec<MenuItem>) -> R4 {
    let resp = ui.interact(r.intersect(clip), ui.id().with(("add_send", t.id.0, r.min.x as i32)), Sense::click());
    let p = ui.painter().with_clip_rect(clip);
    if resp.hovered() {
        p.rect_filled(r, 2.0, TOKENS.button_hover);
    }
    p.text(pos2(r.min.x + 4.0, r.center().y), Align2::LEFT_CENTER, "+ Send", FontId::proportional(10.5), TOKENS.text_dim);
    let mut choices = send_targets(s, t, None);
    choices.push(("New FX Channel…".into(), NEW_FX.into()));
    if let Some(v) = choice_menu(&resp, "", &choices, menu) {
        let _ = app.add_send(t.id, &v);
    }
    r4(r)
}

/// A strip's sends block: `rows` rows at `y`, the sends in slot order, then "+ Send". When the
/// sends don't all fit, the last row says how many more there are (the device chain lists all).
#[allow(clippy::too_many_arguments)]
fn sends_block(
    app: &mut StudioApp,
    ui: &mut Ui,
    s: &Session,
    t: &Track,
    x0: f32,
    w: f32,
    y: f32,
    rows: usize,
    clip: Rect,
    sl: &mut StripLayout,
    menu: &mut Vec<MenuItem>,
) {
    if rows == 0 || !takes_sends(t.kind) {
        return;
    }
    let sends: Vec<(usize, SendSlot)> = t.mixer.sends.iter().enumerate().filter_map(|(i, x)| x.clone().map(|x| (i, x))).collect();
    let free = sends.len() < t.mixer.sends.len();
    let row_r = |i: usize| Rect::from_min_size(pos2(x0, y + i as f32 * SEND_ROW_H), vec2(w, SEND_ROW_H - 2.0));
    let fits = sends.len() + usize::from(free) <= rows;
    let shown = if fits { sends.len() } else { rows.saturating_sub(1) };
    for (i, (slot, snd)) in sends.iter().take(shown).enumerate() {
        let row = send_row(app, ui, s, t, *slot, snd, row_r(i), clip, false, menu);
        sl.sends.push(row);
    }
    if fits {
        if free {
            sl.add_send = add_send_row(app, ui, s, t, row_r(shown), clip, menu);
        }
    } else {
        let r = row_r(shown);
        let resp = ui.interact(r.intersect(clip), ui.id().with(("more_sends", t.id.0)), Sense::click());
        ui.painter().with_clip_rect(clip).text(
            pos2(r.min.x + 4.0, r.center().y),
            Align2::LEFT_CENTER,
            format!("{} more…", sends.len() - shown),
            FontId::proportional(10.5),
            TOKENS.text_dim,
        );
        if resp.on_hover_text("Show all sends in the device chain").clicked() {
            app.focus(t.id);
            app.ui.show_devices = true;
        }
    }
}

/// One channel strip.
#[allow(clippy::too_many_arguments)]
fn strip(
    app: &mut StudioApp,
    ui: &mut Ui,
    s: &Session,
    t: &Track,
    r: Rect,
    clip: Rect,
    focused: bool,
    rows: usize,
    menu: &mut Vec<MenuItem>,
) -> StripLayout {
    let id = t.id;
    let m = &t.mixer;
    let p = ui.painter().with_clip_rect(clip);
    p.rect_filled(r, 3.0, if focused { TOKENS.header_selected } else { TOKENS.panel });
    let mut sl = StripLayout {
        track: id.0,
        name: t.name.clone(),
        kind: Some(t.kind),
        rect: r4(r),
        volume_db: m.volume_db,
        balance: balance_of(&m.pan),
        pan: m.pan.clone(),
        level: app.meters.get(&id).map_or([0.0; 2], |d| d.level),
        fx: t.kind == TrackKind::Aux,
        ..StripLayout::default()
    };
    let x0 = r.min.x + 4.0;
    let w = r.width() - 8.0;
    // Name and color: click focuses the track.
    let name_r = Rect::from_min_size(pos2(r.min.x, r.min.y), vec2(r.width(), 22.0));
    p.rect_filled(Rect::from_min_size(name_r.min, vec2(r.width(), 4.0)), egui::CornerRadius { nw: 3, ne: 3, sw: 0, se: 0 }, theme::rgb(t.color));
    let mut tr = name_r.shrink2(vec2(5.0, 0.0));
    if sl.fx {
        // FX channels: a small "FX" tag and a muted name.
        let tag = Rect::from_min_size(pos2(tr.max.x - 18.0, tr.center().y - 5.0), vec2(18.0, 13.0));
        p.rect_filled(tag, 2.0, TOKENS.button);
        p.text(tag.center(), Align2::CENTER_CENTER, "FX", FontId::proportional(9.5), TOKENS.text_dim);
        tr.max.x = tag.min.x - 3.0;
    }
    p.with_clip_rect(tr.intersect(clip)).text(
        pos2(tr.min.x, tr.center().y + 2.0),
        Align2::LEFT_CENTER,
        &t.name,
        FontId::proportional(13.0),
        if sl.fx { TOKENS.text_dim } else { TOKENS.text },
    );
    sl.name_rect = r4(name_r);
    let resp = ui.interact(name_r.intersect(clip), ui.id().with(("strip_name", id.0)), Sense::click());
    if resp.clicked() {
        app.focus(id);
    }
    let mut y = r.min.y + 26.0;
    // Input (audio and aux tracks; instrument input is MIDI and there is no MIDI device layer).
    let in_r = Rect::from_min_size(pos2(x0, y), vec2(w, 16.0));
    if matches!(t.kind, TrackKind::Audio | TrackKind::Aux) {
        let cur = route_label(s, &m.input, true, if t.kind == TrackKind::Audio { t.channels() } else { 0 });
        let choices = input_choices(s, t, app.device_inputs());
        if let Some(v) = dropdown(ui, clip, in_r, ui.id().with(("input", id.0)), &cur, &choices, menu) {
            let _ = app.set_input(id, &v);
        }
        sl.input = r4(in_r);
        sl.input_label = cur;
    } else {
        let label = if t.kind.is_midi() { "MIDI in" } else { "" };
        p.text(in_r.left_center() + vec2(4.0, 0.0), Align2::LEFT_CENTER, label, FontId::proportional(10.0), TOKENS.text_dim);
    }
    y += 19.0;
    // Sends (Logic: between the input and the output).
    sends_block(app, ui, s, t, x0, w, y, rows, clip, &mut sl, menu);
    y += rows as f32 * SEND_ROW_H;
    // Output.
    let out_r = Rect::from_min_size(pos2(x0, y), vec2(w, 16.0));
    let cur = route_label(s, &m.output, false, t.channels());
    let choices = output_choices(s, t);
    if let Some(v) = dropdown(ui, clip, out_r, ui.id().with(("output", id.0)), &cur, &choices, menu) {
        let _ = app.set_output(id, &v);
    }
    sl.output = r4(out_r);
    sl.output_label = cur;
    y += 20.0;
    y += 4.0;
    // Balance knob.
    let knob = Rect::from_center_size(pos2(r.center().x, y + 18.0), vec2(34.0, 34.0));
    sl.knob = r4(knob);
    let b = balance_of(&m.pan);
    let has_pan = !m.pan.is_empty();
    let resp = ui.interact(knob.intersect(clip), ui.id().with(("knob", id.0)), Sense::click_and_drag());
    draw_knob(&p, knob, b, resp.hovered() || matches!(app.mixer.gesture, Some(MixGesture::Knob { track, .. }) if track == id));
    if has_pan {
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
        }
        if resp.drag_started() {
            app.mixer.gesture = Some(MixGesture::Knob { track: id, start: b });
        }
        if resp.dragged()
            && let Some(MixGesture::Knob { track, start }) = app.mixer.gesture
            && track == id
        {
            let dy = ui.input(|i| i.pointer.press_origin().zip(i.pointer.latest_pos()).map_or(0.0, |(a, b)| b.y - a.y));
            let fine = ui.input(|i| i.modifiers.shift);
            let nb = (start - dy / if fine { 500.0 } else { 100.0 }).clamp(-1.0, 1.0);
            let nb = if nb.abs() < 0.005 { 0.0 } else { nb };
            let _ = app.set_balance(id, nb);
        }
        if resp.drag_stopped() {
            app.mixer.gesture = None;
        }
        if resp.double_clicked() {
            let _ = app.set_balance(id, 0.0);
            app.engine.end_merge();
        }
    }
    let pan_text = if b.abs() < 0.005 { "C".to_string() } else { format!("{}{:.0}", if b < 0.0 { "L" } else { "R" }, b.abs() * 100.0) };
    p.text(pos2(r.center().x, knob.max.y + 7.0), Align2::CENTER_CENTER, pan_text, FontId::proportional(9.5), TOKENS.text_dim);
    y = knob.max.y + 16.0;
    // M S / R I.
    let bw = (w - 3.0) / 2.0;
    let cell = |i: usize, row: f32| Rect::from_min_size(pos2(x0 + (i as f32) * (bw + 3.0), row), vec2(bw, 16.0));
    let buttons: [(&str, bool, Color32, &str, bool, Rect); 4] = [
        ("M", m.mute, TOKENS.mute, "mix.mute", true, cell(0, y)),
        ("S", m.solo, TOKENS.solo, "mix.solo", true, cell(1, y)),
        ("R", m.record_arm, TOKENS.record, "mix.record_arm", t.kind.has_playlist(), cell(0, y + 19.0)),
        ("I", m.input_monitor, TOKENS.monitor, "mix.input_monitor", t.kind == TrackKind::Audio, cell(1, y + 19.0)),
    ];
    if t.kind == TrackKind::Master {
        // A master has no solo, record or input.
        let resp = small_button(ui, clip, cell(0, y), ui.id().with(("mix.mute", id.0)), "M", m.mute, TOKENS.mute, true);
        if resp.clicked() {
            let _ = app.run("mix.mute", json!({"tracks": [id.0], "value": !m.mute}));
        }
        sl.mute = r4(cell(0, y));
    } else {
        for (label, on, color, cmd, enabled, br) in buttons {
            let resp = small_button(ui, clip, br, ui.id().with((cmd, id.0, "strip")), label, on, color, enabled);
            if resp.clicked() && enabled {
                let _ = app.run(cmd, json!({"tracks": [id.0], "value": !on}));
            }
            match label {
                "M" => sl.mute = r4(br),
                "S" => sl.solo = r4(br),
                "R" => sl.arm = r4(br),
                _ => sl.monitor = r4(br),
            }
        }
    }
    y += 42.0;
    // Fader and meter.
    let bottom = r.max.y - 22.0;
    let fader = Rect::from_min_max(pos2(x0 + 4.0, y + 6.0), pos2(r.center().x + 4.0, bottom - 6.0));
    let meter = Rect::from_min_max(pos2(r.center().x + 12.0, y + 6.0), pos2(r.max.x - 10.0, bottom - 6.0));
    sl.fader = r4(fader);
    sl.meter = r4(meter);
    let d = app.meters.get(&id).copied().unwrap_or_default();
    draw_meter(&p, meter, d.level, d.clip);
    fader_widget(app, ui, &p, clip, id, m.volume_db, fader);
    let db = m.volume_db;
    let text = if db <= -144.0 { "-inf".to_string() } else { format!("{db:+.1}") };
    let ro = Rect::from_min_max(pos2(x0, bottom), pos2(r.max.x - 4.0, r.max.y - 4.0));
    p.rect_filled(ro, 2.0, TOKENS.bg);
    p.text(ro.center(), Align2::CENTER_CENTER, text, FontId::monospace(10.5), TOKENS.text);
    sl
}

fn draw_knob(p: &egui::Painter, r: Rect, b: f32, hot: bool) {
    // Flat: a thin ring for the travel and the accent from the center to the value.
    let c = r.center();
    let rad = r.width() / 2.0 - 4.0;
    let ang = |v: f32| (-90.0 + v * 135.0_f32).to_radians();
    let arc = |from: f32, to: f32| -> Vec<egui::Pos2> {
        let n = 24;
        (0..=n).map(|i| from + (to - from) * i as f32 / n as f32).map(|v| c + vec2(ang(v).cos(), ang(v).sin()) * rad).collect()
    };
    p.add(egui::Shape::line(arc(-1.0, 1.0), Stroke::new(3.0, TOKENS.button)));
    p.add(egui::Shape::line(arc(0.0, b), Stroke::new(3.0, if hot { theme::lighten(TOKENS.accent, 0.15) } else { TOKENS.accent })));
    p.circle_filled(c + vec2(ang(b).cos(), ang(b).sin()) * rad, 2.5, TOKENS.text);
}

#[allow(clippy::too_many_arguments)]
fn fader_widget(app: &mut StudioApp, ui: &mut Ui, p: &egui::Painter, clip: Rect, id: TrackId, db: f32, r: Rect) {
    let resp = ui.interact(r.expand2(vec2(4.0, 6.0)).intersect(clip), ui.id().with(("fader", id.0)), Sense::click_and_drag());
    // Track and 0 dB tick.
    let groove = Rect::from_center_size(r.center(), vec2(4.0, r.height()));
    p.rect_filled(groove, 2.0, TOKENS.bg);
    let y_of = |k: f32| r.max.y - k * r.height();
    for mark in [12.0, 6.0, 0.0, -6.0, -12.0, -24.0, -48.0] {
        let yy = y_of(db_to_k(mark));
        p.line_segment(
            [pos2(r.min.x, yy), pos2(r.min.x + 5.0, yy)],
            Stroke::new(1.0, TOKENS.text_dim.gamma_multiply(if mark == 0.0 { 1.0 } else { 0.5 })),
        );
    }
    let k = db_to_k(db);
    let cap = Rect::from_center_size(pos2(r.center().x, y_of(k)), vec2(r.width().min(26.0), 12.0));
    let hot = resp.hovered() || matches!(app.mixer.gesture, Some(MixGesture::Fader { track, .. }) if track == id);
    p.rect_filled(cap, 2.0, if hot { TOKENS.accent } else { TOKENS.text_dim });
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    }
    if resp.drag_started() {
        app.mixer.gesture = Some(MixGesture::Fader { track: id, start_k: k, height: r.height().max(1.0) });
    }
    if resp.dragged()
        && let Some(MixGesture::Fader { track, start_k, height }) = app.mixer.gesture
        && track == id
    {
        let dy = ui.input(|i| i.pointer.press_origin().zip(i.pointer.latest_pos()).map_or(0.0, |(a, b)| b.y - a.y));
        let fine = ui.input(|i| i.modifiers.shift);
        let nk = (start_k - dy / height * if fine { 0.2 } else { 1.0 }).clamp(0.0, 1.0);
        let _ = app.set_volume_merged(id, k_to_db(nk));
    }
    if resp.drag_stopped() {
        app.mixer.gesture = None;
    }
    if resp.double_clicked() {
        let _ = app.run("mix.volume", json!({"tracks": [id.0], "db": 0.0}));
    }
}
