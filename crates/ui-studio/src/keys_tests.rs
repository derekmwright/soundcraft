//! Piano roll (slice 3) tests. Every gesture is driven the way a user drives it: synthetic
//! clicks, drags, wheel and keys through the control channel's input queue, one frame at a time.
//! No player, no plug-ins, no scan.

use crate::keys::Snap;
use crate::{Services, StudioApp};
use serde_json::{Value, json};
use soundcraft_midi::Note;
use soundcraft_model::{ClipContent, ClipId, TrackId};

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    track: TrackId,
    clip: ClipId,
}

impl T {
    /// An instrument track with a one-bar MIDI clip at bar 1, the mixer pane shown.
    fn new() -> T {
        let mut app = StudioApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        let r = app.run("track.new", json!({"kind": "instrument"})).unwrap();
        let track = TrackId(r["tracks"][0].as_u64().unwrap());
        let c = app.run("midi.new_clip", json!({"tracks": [track.0], "start": 0, "end": 0})).unwrap();
        let clip = ClipId(c["clip"].as_u64().unwrap());
        app.ui.show_mixer = true;
        app.ui.show_devices = false;
        let mut t = T { app, ctx: egui::Context::default(), time: 0.0, track, clip };
        t.frames(3);
        t
    }

    /// Opened by a double-click on the clip in the arrangement.
    fn open() -> T {
        let mut t = T::new();
        let r = t.app.arrange.layout.clips.iter().find(|c| c.id == t.clip.0).unwrap().rect;
        t.click_n(mid(r), 2, json!({}));
        assert!(t.app.keys.layout.visible, "double-click opened the piano roll");
        t
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0))),
                time: Some(self.time),
                ..Default::default()
            };
            self.app.raw_input_hook(&mut raw);
            let app = &mut self.app;
            let mut out = self.ctx.run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
        }
    }

    fn input(&mut self, method: &str, p: Value) {
        let (req, _rx) = crate::ControlRequest::new(method, p);
        let ctx = egui::Context::default();
        let _ = crate::control::handle_for_test(&mut self.app, &ctx, &req);
        self.frames(30);
    }

    fn click_n(&mut self, at: (f32, f32), count: u64, mods: Value) {
        self.input("ui.click", merged(json!({"x": at.0, "y": at.1, "count": count}), mods));
    }

    fn click(&mut self, at: (f32, f32)) {
        self.click_n(at, 1, json!({}));
    }

    fn drag(&mut self, from: (f32, f32), to: (f32, f32), mods: Value) {
        self.input("ui.drag", merged(json!({"x": from.0, "y": from.1, "to_x": to.0, "to_y": to.1}), mods));
    }

    fn key(&mut self, key: &str, mods: Value) {
        self.input("ui.key", merged(json!({"key": key}), mods));
    }

    /// The point on the grid at `tick` (from the clip start) in the middle of `pitch`'s row.
    fn at(&self, tick: f64, pitch: u8) -> (f32, f32) {
        let l = &self.app.keys.layout;
        let x = l.grid[0] + ((tick - l.scroll_ticks) * l.px_per_tick) as f32;
        let y = l.grid[1] + (l.top_pitch - f32::from(pitch)) * l.key_h + l.key_h / 2.0;
        (x, y)
    }

    fn notes(&self) -> Vec<Note> {
        match &self.app.engine.session().find_clip(self.clip).unwrap().1.content {
            ClipContent::Midi { sequence } => sequence.notes.clone(),
            _ => Vec::new(),
        }
    }

    fn undo_len(&self) -> usize {
        self.app.engine.undo_history().len()
    }

    fn add(&mut self, pitch: u8, start: i64, length: i64) {
        self.app.run("midi.note_add", json!({"clip": self.clip.0, "pitch": pitch, "start_ticks": start, "length_ticks": length})).unwrap();
        self.frames(2);
    }

    fn note_rect(&self, pitch: u8, start: i64) -> [f32; 4] {
        self.app.keys.layout.notes.iter().find(|n| n.pitch == pitch && n.start == start).map(|n| n.rect).unwrap()
    }

    fn selected(&self) -> usize {
        self.app.keys.layout.selected
    }
}

fn merged(mut a: Value, b: Value) -> Value {
    if let (Some(o), Some(m)) = (a.as_object_mut(), b.as_object()) {
        o.extend(m.clone());
    }
    a
}

fn mid(r: [f32; 4]) -> (f32, f32) {
    ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
}

#[test]
fn a_double_click_on_a_midi_clip_opens_the_piano_roll_and_tabs_shortcut_and_escape_switch_back() {
    let mut t = T::new();
    assert!(t.app.mixer.layout.visible, "the mixer pane to start with");
    let undo = t.undo_len();
    let r = t.app.arrange.layout.clips.iter().find(|c| c.id == t.clip.0).unwrap().rect;
    t.click_n(mid(r), 2, json!({}));
    let l = t.app.keys.layout.clone();
    assert!(l.visible && l.clip == Some(t.clip.0) && l.track == Some(t.track.0), "{l:?}");
    assert!(!t.app.mixer.layout.visible, "the piano roll replaces the mixer pane");
    assert!(l.pane[1] > t.app.arrange.layout.timeline[1], "in the bottom pane");
    // Devices tab: the device chain comes back in its place.
    t.click(mid(l.tab_devices));
    assert!(!t.app.keys.layout.visible && t.app.ui.show_devices && !t.app.ui.show_mixer);
    // Ctrl+Alt+K brings the same clip back.
    t.key("K", json!({"cmd": true, "alt": true}));
    assert!(t.app.keys.layout.visible && t.app.keys.layout.clip == Some(t.clip.0));
    t.click(mid(t.app.keys.layout.tab_mixer));
    assert!(!t.app.keys.layout.visible && t.app.mixer.layout.visible, "Mixer tab");
    t.key("K", json!({"cmd": true, "alt": true}));
    assert!(t.app.keys.layout.visible);
    // Escape in the piano roll returns to the mixer that was shown before.
    t.key("Escape", json!({}));
    assert!(!t.app.keys.layout.visible && t.app.mixer.layout.visible, "Escape");
    assert_eq!(t.undo_len(), undo, "opening and closing is view state");
}

#[test]
fn a_double_click_on_the_grid_adds_a_snapped_note_and_a_double_click_on_it_deletes_it() {
    let mut t = T::open();
    let undo = t.undo_len();
    // Just after the first sixteenth: the note goes in that cell, a sixteenth long, velocity 100.
    t.click_n(t.at(300.0, 62), 2, json!({}));
    let notes = t.notes();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!((notes[0].pitch, notes[0].start, notes[0].length, notes[0].velocity), (62, 240, 240, 100));
    assert_eq!(t.undo_len(), undo + 1, "one undo entry");
    assert_eq!(t.app.keys.layout.last_audition, Some((62, 100)), "placing auditions it");
    assert_eq!(t.selected(), 1, "the new note is selected");
    t.click_n(mid(t.note_rect(62, 240)), 2, json!({}));
    assert!(t.notes().is_empty(), "double-click on a note deletes it");
    assert_eq!(t.undo_len(), undo + 2);
}

#[test]
fn click_shift_click_rubber_band_ctrl_a_and_delete_select_and_remove() {
    let mut t = T::open();
    t.add(60, 0, 240);
    t.add(64, 480, 240);
    t.add(67, 960, 240);
    t.click(mid(t.note_rect(60, 0)));
    assert_eq!(t.selected(), 1);
    t.click_n(mid(t.note_rect(64, 480)), 1, json!({"shift": true}));
    assert_eq!(t.selected(), 2, "Shift-click adds");
    t.click(t.at(1800.0, 62));
    assert_eq!(t.selected(), 0, "a click on empty grid clears the selection");
    // Rubber band over the first two notes, from empty space.
    let from = t.at(1.0, 66);
    let to = t.at(700.0, 58);
    t.drag(from, to, json!({}));
    assert_eq!(t.selected(), 2, "rubber band");
    let undo = t.undo_len();
    t.key("Delete", json!({}));
    let left: Vec<u8> = t.notes().iter().map(|n| n.pitch).collect();
    assert_eq!(left, vec![67], "Delete removes the selection");
    assert_eq!(t.undo_len(), undo + 1);
    t.key("A", json!({"cmd": true}));
    assert_eq!(t.selected(), 1, "Ctrl+A");
    t.key("Backspace", json!({}));
    assert!(t.notes().is_empty());
    assert_eq!(t.app.arrange.layout.clips.len(), 1, "the arrangement's Delete did not run: the clip stays");
}

#[test]
fn dragging_moves_snapped_the_edge_resizes_and_ctrl_drag_copies_a_note_or_the_selection() {
    let mut t = T::open();
    t.add(60, 0, 480);
    let undo = t.undo_len();
    // Move: about two sixteenths and a bit later, two keys up.
    let grab = t.at(120.0, 60);
    let to = t.at(120.0 + 510.0, 62);
    t.drag(grab, to, json!({}));
    let n = t.notes();
    assert_eq!((n.len(), n[0].start, n[0].pitch, n[0].length), (1, 480, 62, 480), "{n:?}");
    assert_eq!(t.undo_len(), undo + 1, "a move is one undo entry");
    // Resize from the right edge by a sixteenth and a bit.
    let r = t.note_rect(62, 480);
    let px = 260.0 * t.app.keys.layout.px_per_tick as f32;
    t.drag((r[2] - 2.0, (r[1] + r[3]) / 2.0), (r[2] - 2.0 + px, (r[1] + r[3]) / 2.0), json!({}));
    let n = t.notes();
    assert_eq!((n[0].start, n[0].length), (480, 720), "{n:?}");
    assert_eq!(t.undo_len(), undo + 2);
    // Ctrl+drag copies one bar later; the original stays.
    let grab = t.at(600.0, 62);
    let to = t.at(600.0 + 3840.0, 62);
    t.drag(grab, to, json!({"cmd": true}));
    let n = t.notes();
    assert_eq!(n.iter().map(|n| (n.start, n.pitch)).collect::<Vec<_>>(), vec![(480, 62), (4320, 62)], "{n:?}");
    assert_eq!(t.undo_len(), undo + 3, "a copy is one undo entry");
    // Highlight both and Ctrl+drag the selection down an octave: two more notes in one step.
    let a = t.at(300.0, 64);
    let b = t.at(5400.0, 61);
    t.drag(a, b, json!({}));
    assert_eq!(t.selected(), 2);
    let grab = t.at(600.0, 62);
    let to = t.at(600.0, 50);
    t.drag(grab, to, json!({"cmd": true}));
    let mut pitches: Vec<u8> = t.notes().iter().map(|n| n.pitch).collect();
    pitches.sort();
    assert_eq!(pitches, vec![50, 50, 62, 62]);
    assert_eq!(t.undo_len(), undo + 4, "copying a selection is one undo entry");
    assert_eq!(t.selected(), 2, "the copies are selected");
    assert!(t.app.keys.layout.notes.iter().filter(|n| n.selected).all(|n| n.pitch == 50));
    t.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(t.notes().len(), 2, "undo removes both copies");
}

#[test]
fn alt_drag_and_the_velocity_lane_set_velocity_in_one_step() {
    let mut t = T::open();
    t.add(60, 0, 480);
    let undo = t.undo_len();
    let grab = t.at(200.0, 60);
    t.drag(grab, (grab.0, grab.1 - 30.0), json!({"alt": true}));
    let n = t.notes();
    assert_eq!((n[0].velocity, n[0].start, n[0].pitch), (120, 0, 60), "Alt+drag up 30 points: +20");
    assert_eq!(t.undo_len(), undo + 1);
    assert_eq!(t.app.ui.keys.velocity, 120, "the next note uses the last velocity");
    // The velocity lane: drag the stem to about a quarter height.
    let l = t.app.keys.layout.clone();
    let stem = l.notes[0].vel_rect;
    let y = l.velocity[3] - (l.velocity[3] - l.velocity[1] - 6.0) * 32.0 / 127.0;
    t.drag((stem[0] + 1.0, stem[1] + 2.0), (stem[0] + 1.0, y), json!({}));
    let v = t.notes()[0].velocity;
    assert!((30..=34).contains(&v), "lane drag set {v}");
    assert_eq!(t.undo_len(), undo + 2);
    // A click in the lane sets it at that height.
    let y = l.velocity[3] - (l.velocity[3] - l.velocity[1] - 6.0) * 100.0 / 127.0;
    t.click((stem[0] + 1.0, y));
    let v = t.notes()[0].velocity;
    assert!((98..=102).contains(&v), "lane click set {v}");
    // A new note takes the last velocity.
    t.click_n(t.at(1000.0, 64), 2, json!({}));
    let new = t.notes().into_iter().find(|n| n.pitch == 64).unwrap();
    assert_eq!(new.velocity, v);
}

#[test]
fn ctrl_d_arrows_ctrl_up_and_ctrl_u_edit_the_selection() {
    let mut t = T::open();
    t.add(60, 0, 240);
    t.click(mid(t.note_rect(60, 0)));
    let undo = t.undo_len();
    t.key("D", json!({"cmd": true}));
    let n = t.notes();
    assert_eq!(n.iter().map(|n| n.start).collect::<Vec<_>>(), vec![0, 240], "Ctrl+D: one grid length after itself");
    assert_eq!(t.undo_len(), undo + 1);
    assert_eq!(t.app.keys.selection, vec![n[1]], "the copy is selected");
    assert_eq!(t.app.arrange.layout.clips.len(), 1, "the arrangement's Ctrl+D did not run");
    t.key("ArrowRight", json!({}));
    t.key("ArrowUp", json!({}));
    t.key("ArrowUp", json!({"shift": true}));
    let moved = t.app.keys.selection[0];
    assert_eq!((moved.start, moved.pitch), (480, 73), "right a sixteenth, up one, up an octave");
    assert_eq!(t.undo_len(), undo + 4, "each nudge is one step");
    // Ctrl+Up with everything selected: event.transpose on the clip.
    t.key("A", json!({"cmd": true}));
    t.key("ArrowUp", json!({"cmd": true}));
    let mut p: Vec<u8> = t.notes().iter().map(|n| n.pitch).collect();
    p.sort();
    assert_eq!(p, vec![72, 85]);
    assert_eq!(t.app.engine.undo_history().last().map(String::as_str), Some("Transpose..."));
    assert_eq!(t.selected(), 2, "the selection is read back after the edit");
    // Ctrl+U on one off-grid note of three: only that one moves.
    t.add(62, 1000, 200);
    t.click(mid(t.note_rect(62, 1000)));
    t.key("U", json!({"cmd": true}));
    let n = t.notes();
    assert!(n.iter().any(|n| n.pitch == 62 && n.start == 960), "{n:?}");
    assert!(n.iter().any(|n| n.pitch == 85 && n.start == 480), "the other notes stay");
    // The Quantize button with everything selected runs event.quantize.
    t.app.run("midi.notes_move", json!({"clip": t.clip.0, "indices": [0, 1, 2], "ticks": 30})).unwrap();
    t.frames(2);
    t.key("A", json!({"cmd": true}));
    t.click(mid(t.app.keys.layout.quantize));
    assert_eq!(t.app.engine.undo_history().last().map(String::as_str), Some("Quantize..."));
    assert!(t.notes().iter().all(|n| n.start % 240 == 0), "{:?}", t.notes());
}

#[test]
fn the_audition_switch_and_the_keyboard_column() {
    let mut t = T::open();
    assert!(t.app.ui.keys.audition, "on by default");
    t.click(mid(t.app.keys.layout.audition));
    assert!(!t.app.ui.keys.audition);
    let before = t.app.keys.layout.auditions;
    t.click_n(t.at(10.0, 60), 2, json!({}));
    assert_eq!(t.notes().len(), 1);
    assert_eq!(t.app.keys.layout.auditions, before, "off: placing a note is silent");
    t.click(mid(t.app.keys.layout.audition));
    assert!(t.app.ui.keys.audition, "and back on");
    t.click(mid(t.note_rect(60, 0)));
    assert_eq!(t.app.keys.layout.auditions, before + 1, "clicking a note plays it");
    // A key in the keyboard column plays its pitch.
    let kb = t.app.keys.layout.keyboard;
    let (_, y) = t.at(0.0, 65);
    t.click(((kb[0] + kb[2]) / 2.0, y));
    assert_eq!(t.app.keys.layout.last_audition.map(|a| a.0), Some(65));
    assert_eq!(t.app.keys.layout.sounding, Vec::<u8>::new(), "released after the click");
}

#[test]
fn view_gestures_change_the_ui_state_and_never_the_document() {
    let mut t = T::open();
    let doc = serde_json::to_value(t.app.engine.session()).unwrap();
    let undo = t.undo_len();
    // Grid menu.
    t.click(mid(t.app.keys.layout.snap));
    let item = t.app.keys.layout.snap_menu.iter().find(|m| m.label == "1/8").cloned().expect("1/8 listed");
    assert_eq!(t.app.keys.layout.snap_menu.len(), Snap::ALL.len());
    t.click(mid(item.rect));
    assert_eq!(t.app.ui.keys.snap, Snap::Eighth);
    // Pane height: drag the top edge up.
    let h0 = t.app.ui.keys.height;
    let hd = mid(t.app.keys.layout.handle);
    t.drag(hd, (hd.0, hd.1 - 60.0), json!({}));
    assert!((t.app.ui.keys.height - (h0 + 60.0)).abs() < 1.0, "{}", t.app.ui.keys.height);
    assert!((t.app.keys.layout.pane[3] - t.app.keys.layout.pane[1] - t.app.ui.keys.height).abs() < 1.5);
    // Ctrl+wheel zooms time; Ctrl+Shift+wheel and Alt+wheel zoom the keys.
    let g = mid(t.app.keys.layout.grid);
    let z0 = t.app.ui.keys.px_per_quarter;
    t.input("ui.scroll", json!({"x": g.0, "y": g.1, "dy": 120, "cmd": true}));
    assert!(t.app.ui.keys.px_per_quarter > z0, "Ctrl+wheel zoomed in: {}", t.app.ui.keys.px_per_quarter);
    let k0 = t.app.ui.keys.key_h;
    t.input("ui.scroll", json!({"x": g.0, "y": g.1, "dy": 120, "cmd": true, "shift": true}));
    let k1 = t.app.ui.keys.key_h;
    assert!(k1 > k0, "Ctrl+Shift+wheel: {k0} -> {k1}");
    t.input("ui.scroll", json!({"x": g.0, "y": g.1, "dy": -120, "alt": true}));
    assert!(t.app.ui.keys.key_h < k1, "Alt+wheel: {k1} -> {}", t.app.ui.keys.key_h);
    // Plain wheel scrolls the keys.
    let top = t.app.keys.layout.top_pitch;
    t.input("ui.scroll", json!({"x": g.0, "y": g.1, "dy": 80}));
    assert!(t.app.keys.layout.top_pitch > top, "wheel scrolled up");
    // Ruler drag down zooms time in; keyboard drag sideways zooms the keys.
    let z1 = t.app.ui.keys.px_per_quarter;
    let r = mid(t.app.keys.layout.ruler);
    t.drag(r, (r.0, r.1 + 40.0), json!({}));
    assert!(t.app.ui.keys.px_per_quarter > z1 * 1.3, "ruler drag: {z1} -> {}", t.app.ui.keys.px_per_quarter);
    let k2 = t.app.ui.keys.key_h;
    let kb = mid(t.app.keys.layout.keyboard);
    t.drag(kb, (kb.0 + 30.0, kb.1), json!({}));
    assert!(t.app.ui.keys.key_h > k2, "keyboard drag: {k2} -> {}", t.app.ui.keys.key_h);
    // A ruler click moves the cursor (view state too).
    let r = t.app.keys.layout.ruler;
    t.click((r[0] + 5.0, (r[1] + r[3]) / 2.0));
    assert_eq!(t.undo_len(), undo, "no undo entries");
    assert_eq!(serde_json::to_value(t.app.engine.session()).unwrap(), doc, "the document is untouched");
    // The preferences go out with ui.inspect and come back through ui.set.
    let v = t.app.inspect(&t.ctx);
    assert_eq!(v["ui"]["keys"]["snap"], json!("1/8"));
    t.app.run("ui.set", json!({"keys": {"snap": "1/32", "height": 300.0, "audition": false}})).unwrap();
    assert_eq!((t.app.ui.keys.snap, t.app.ui.keys.audition), (Snap::ThirtySecond, false));
}

#[test]
fn the_playhead_follows_and_the_editor_stays_on_its_clip() {
    let mut t = T::open();
    t.app.keys.scroll = 20_000.0;
    t.frames(2);
    t.app.run("transport.play", json!({"from": 0})).unwrap();
    t.frames(10);
    assert!(t.app.is_playing());
    let l = &t.app.keys.layout;
    assert!(l.scroll_ticks < 20_000.0, "the view paged to the playhead: {}", l.scroll_ticks);
    assert_eq!(l.clip, Some(t.clip.0));
    t.key("Space", json!({}));
    assert!(!t.app.is_playing(), "Space still toggles the transport");
    // Undoing the clip away closes the editor.
    while t.app.engine.session().find_clip(t.clip).is_some() {
        t.app.run("edit.undo", json!({})).unwrap();
    }
    t.frames(2);
    assert!(!t.app.keys.layout.visible && t.app.keys.clip.is_none());
}
