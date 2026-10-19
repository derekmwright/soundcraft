//! Track groups and reordering (slice 2c) tests. Every gesture is driven the way a user drives
//! it: synthetic clicks, drags and keys through the control channel's input queue, one frame at a
//! time. No player, no plug-ins, no scan.

use crate::{Services, StudioApp};
use serde_json::{Value, json};
use soundcraft_model::{TrackId, TrackKind};

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    /// Audio tracks "A", "B", "C", in that order.
    a: TrackId,
    b: TrackId,
    c: TrackId,
}

impl T {
    /// Three audio tracks, the mixer pane shown, the document clean.
    fn new() -> T {
        let mut app = StudioApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        let mut ids = Vec::new();
        for name in ["A", "B", "C"] {
            let r = app.run("track.new", json!({"kind": "audio", "name": name})).unwrap();
            ids.push(TrackId(r["tracks"][0].as_u64().unwrap()));
        }
        app.ui.show_mixer = true;
        app.ui.show_devices = true;
        app.engine.mark_clean();
        let mut t = T { app, ctx: egui::Context::default(), time: 0.0, a: ids[0], b: ids[1], c: ids[2] };
        t.frames(3);
        t
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))),
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

    fn click(&mut self, at: (f32, f32), mods: Value) {
        self.input("ui.click", merged(json!({"x": at.0, "y": at.1}), mods));
    }

    fn right_click(&mut self, at: (f32, f32)) {
        self.input("ui.click", json!({"x": at.0, "y": at.1, "button": "right"}));
    }

    fn drag(&mut self, from: (f32, f32), to: (f32, f32)) {
        self.input("ui.drag", json!({"x": from.0, "y": from.1, "to_x": to.0, "to_y": to.1, "steps": 12}));
    }

    fn key(&mut self, key: &str, mods: Value) {
        self.input("ui.key", merged(json!({"key": key}), mods));
    }

    fn row(&self, t: TrackId) -> crate::arrange::TrackRow {
        self.app.arrange.layout.tracks.iter().find(|r| r.id == t.0).cloned().unwrap_or_else(|| panic!("no row for {t:?}"))
    }

    /// A point on the track's header name area, `f` of the way down the row.
    fn header(&self, t: TrackId, f: f32) -> (f32, f32) {
        let h = self.row(t).header;
        (h[0] + 60.0, h[1] + (h[3] - h[1]) * f)
    }

    /// Track names in session order.
    fn order(&self) -> Vec<String> {
        self.app.engine.session().tracks.iter().map(|t| t.name.clone()).collect()
    }

    /// Track names as the arrangement shows them.
    fn rows(&self) -> Vec<String> {
        self.app.arrange.layout.tracks.iter().map(|r| r.name.clone()).collect()
    }

    /// Track names as the mixer shows them.
    fn strips(&self) -> Vec<String> {
        self.app.mixer.layout.strips.iter().map(|s| s.name.clone()).collect()
    }

    fn undo_len(&self) -> usize {
        self.app.engine.undo_history().len()
    }

    fn folder_of(&self, t: TrackId) -> Option<TrackId> {
        self.app.engine.session().track(t).unwrap().folder
    }

    fn groups(&self) -> Vec<TrackId> {
        self.app.engine.session().tracks.iter().filter(|t| t.kind == TrackKind::Folder).map(|t| t.id).collect()
    }

    /// Click A, Shift-click B, Ctrl+G: returns the group.
    fn group_ab(&mut self) -> TrackId {
        self.click(self.header(self.a, 0.5), json!({}));
        self.click(self.header(self.b, 0.5), json!({"shift": true}));
        self.key("G", json!({"cmd": true}));
        let g = self.groups();
        assert_eq!(g.len(), 1, "one group: {:?}", self.order());
        g[0]
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
fn dragging_a_header_reorders_the_tracks_and_the_mixer_follows() {
    let mut t = T::new();
    assert_eq!(t.strips(), ["A", "B", "C"]);
    let u0 = t.undo_len();
    // C above A: drop in the top quarter of A's row.
    t.drag(t.header(t.c, 0.5), t.header(t.a, 0.1));
    assert_eq!(t.order(), ["C", "A", "B"]);
    assert_eq!(t.rows(), ["C", "A", "B"], "the arrangement shows the new order");
    assert_eq!(t.strips(), ["C", "A", "B"], "the mixer strips follow the arrangement");
    assert_eq!(t.undo_len(), u0 + 1, "one undo entry: {:?}", t.app.engine.undo_history());
    // A below B: drop in the bottom quarter of B's row.
    t.drag(t.header(t.a, 0.5), t.header(t.b, 0.9));
    assert_eq!(t.order(), ["C", "B", "A"]);
    assert_eq!(t.undo_len(), u0 + 2);
    t.app.run("edit.undo", json!({})).unwrap();
    t.frames(2);
    assert_eq!(t.order(), ["C", "A", "B"], "undo takes the whole move back");
    // Dropped where it already is: nothing happens.
    t.drag(t.header(t.a, 0.5), t.header(t.a, 0.9));
    assert_eq!(t.order(), ["C", "A", "B"]);
    assert_eq!(t.undo_len(), u0 + 1);
}

#[test]
fn a_header_drag_shows_a_line_where_the_track_would_land() {
    let mut t = T::new();
    let from = t.header(t.c, 0.5);
    let to = t.header(t.a, 0.1);
    // Press and move without letting go.
    let press =
        egui::Event::PointerButton { pos: from.into(), button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE };
    t.app.synthetic.push(egui::Event::PointerMoved(from.into()));
    t.app.synthetic.push(press);
    for k in 1..=10 {
        let f = k as f32 / 10.0;
        t.app.synthetic.push(egui::Event::PointerMoved(egui::pos2(from.0 + (to.0 - from.0) * f, from.1 + (to.1 - from.1) * f)));
    }
    t.frames(14);
    let line = t.app.arrange.layout.drop.expect("a drop line while dragging");
    let a = t.row(t.a).header;
    assert!((line[1] + line[3]) / 2.0 - a[1] < 2.0 && line[3] - line[1] <= 3.0, "a thin line at A's top edge: {line:?} vs {a:?}");
    assert!(line[2] > a[2], "across the lane too");
    assert_eq!(t.order(), ["A", "B", "C"], "nothing moves before the release");
    t.app.synthetic.push(egui::Event::PointerButton {
        pos: to.into(),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    t.frames(10);
    assert!(t.app.arrange.layout.drop.is_none());
    assert_eq!(t.order(), ["C", "A", "B"]);
}

#[test]
fn ctrl_g_groups_the_selected_tracks_with_a_strip_and_indented_members() {
    let mut t = T::new();
    let u0 = t.undo_len();
    let g = t.group_ab();
    let s = t.app.engine.session();
    let folder = s.track(g).unwrap();
    assert_eq!(folder.name, "Group 1");
    assert_eq!(t.folder_of(t.a), Some(g));
    assert_eq!(t.folder_of(t.b), Some(g));
    assert_eq!(t.folder_of(t.c), None);
    assert_eq!(t.undo_len(), u0 + 1, "one undo entry: {:?}", t.app.engine.undo_history());
    assert_eq!(t.order(), ["Group 1", "A", "B", "C"]);
    assert_eq!(t.app.focus_track(), Some(g), "the new group is focused");
    assert_eq!(t.app.mixer.devices.track, Some(g.0), "the device chain follows it: group effects go on its inserts");
    // Members indented under the group; the group has a disclosure triangle.
    assert_eq!((t.row(t.a).depth, t.row(t.b).depth, t.row(t.c).depth, t.row(g).depth), (1, 1, 0, 0));
    assert_ne!(t.row(g).disclosure, [0.0; 4]);
    assert_eq!(t.row(g).subtitle, "Group · 2 tracks");
    // The mixer: the group's strip before its members, an accent bar over all three.
    assert_eq!(t.strips(), ["Group 1", "A", "B", "C"]);
    let link = &t.app.mixer.layout.groups;
    assert_eq!(link.len(), 1);
    assert_eq!(link[0].members, vec![t.a.0, t.b.0]);
    let strip = |n: &str| t.app.mixer.layout.strips.iter().find(|s| s.name == n).unwrap().rect;
    assert!(link[0].rect[0] <= strip("Group 1")[0] + 3.0 && link[0].rect[2] >= strip("B")[2] - 3.0 && link[0].rect[2] < strip("C")[0]);
    // The child's output reads as the group (its bus), and the group strip takes sends.
    let a_out = t.app.mixer.layout.strips.iter().find(|s| s.name == "A").unwrap().output_label.clone();
    assert_eq!(a_out, "Group 1");
    assert!(crate::mixer::takes_sends(TrackKind::Folder));
    // Ctrl+G again with nothing new: a second group, "Group 2".
    t.click(t.header(t.c, 0.5), json!({}));
    t.key("G", json!({"cmd": true}));
    assert!(t.app.engine.session().track_by_name("Group 2").is_some());
}

#[test]
fn collapsing_a_group_is_view_state_and_summarizes_its_members() {
    let mut t = T::new();
    t.app.run("midi.new_clip", json!({"track": t.a.0, "start": 0, "end": 0})).ok();
    let g = t.group_ab();
    t.app.engine.mark_clean();
    let u0 = t.undo_len();
    t.click(mid(t.row(g).disclosure), json!({}));
    assert!(t.row(g).collapsed);
    assert_eq!(t.rows(), ["Group 1", "C"], "members leave the track list");
    assert_eq!(t.strips(), ["Group 1", "C"], "and the mixer");
    assert_eq!(t.app.ui.collapsed, vec![g]);
    assert_eq!(t.undo_len(), u0, "no undo entry");
    assert!(!t.app.engine.is_dirty(), "the document stays clean");
    assert!(t.app.engine.session().track(g).unwrap().folder_open, "the document's folder_open is untouched");
    // The summary lane is read-only: no clip rects there.
    assert!(t.app.arrange.layout.clips.iter().all(|c| c.track != t.a.0));
    t.click(mid(t.row(g).disclosure), json!({}));
    assert_eq!(t.rows(), ["Group 1", "A", "B", "C"]);
    assert!(!t.app.engine.is_dirty() && t.undo_len() == u0);
}

#[test]
fn a_collapsed_lane_draws_member_clips_as_bars() {
    let mut t = T::new();
    let r = t.app.run("track.new", json!({"kind": "instrument", "name": "Keys"})).unwrap();
    let keys = TrackId(r["tracks"][0].as_u64().unwrap());
    t.app.run("midi.new_clip", json!({"track": keys.0, "start": 0, "end": 0})).unwrap();
    t.frames(2);
    t.app.group_tracks(&[t.c, keys]).unwrap();
    t.frames(2);
    let g = t.groups()[0];
    assert_eq!(t.row(g).summary_bars, 0, "expanded: no summary");
    t.click(mid(t.row(g).disclosure), json!({}));
    assert_eq!(t.row(g).summary_bars, 1, "collapsed: one bar for the one member clip");
}

#[test]
fn dragging_into_and_out_of_a_group() {
    let mut t = T::new();
    let g = t.group_ab();
    let u0 = t.undo_len();
    // C onto the group's header: it joins as the last member.
    t.drag(t.header(t.c, 0.5), t.header(g, 0.5));
    assert_eq!(t.folder_of(t.c), Some(g));
    assert_eq!(t.order(), ["Group 1", "A", "B", "C"]);
    assert_eq!(t.undo_len(), u0 + 1);
    assert_eq!(t.row(t.c).depth, 1);
    // A out, below the group (the bottom edge of its last member).
    t.drag(t.header(t.a, 0.5), t.header(t.c, 0.95));
    assert_eq!(t.folder_of(t.a), None);
    assert_eq!(t.order(), ["Group 1", "B", "C", "A"]);
    assert_eq!(t.undo_len(), u0 + 2);
    let out = &t.app.engine.session().track(t.a).unwrap().mixer.output;
    assert_eq!(*out, soundcraft_model::Route::Main, "out of the group, out of its bus");
    // A back in between the members (the top of C's row is inside the group).
    t.drag(t.header(t.a, 0.5), t.header(t.c, 0.1));
    assert_eq!(t.folder_of(t.a), Some(g));
    assert_eq!(t.order(), ["Group 1", "B", "A", "C"]);
    // B out above the group.
    t.drag(t.header(t.b, 0.5), t.header(g, 0.1));
    assert_eq!(t.folder_of(t.b), None);
    assert_eq!(t.order(), ["B", "Group 1", "A", "C"]);
    assert_eq!(t.undo_len(), u0 + 4);
}

#[test]
fn dragging_a_group_moves_its_members_with_it() {
    let mut t = T::new();
    let g = t.group_ab();
    t.drag(t.header(g, 0.1), t.header(t.c, 0.95));
    assert_eq!(t.order(), ["C", "Group 1", "A", "B"]);
    // Never into itself.
    t.drag(t.header(g, 0.1), t.header(t.a, 0.9));
    assert_eq!(t.order(), ["C", "Group 1", "A", "B"]);
}

#[test]
fn ungroup_keeps_the_members_in_place_as_one_undo_step() {
    let mut t = T::new();
    let g = t.group_ab();
    let bus = match t.app.engine.session().track(g).unwrap().mixer.input {
        soundcraft_model::Route::Bus(b) => b,
        _ => panic!("a group listens to its bus"),
    };
    let u0 = t.undo_len();
    t.key("G", json!({"cmd": true, "shift": true}));
    assert!(t.groups().is_empty(), "no group left");
    assert_eq!(t.order(), ["A", "B", "C"]);
    assert_eq!(t.undo_len(), u0 + 1, "one undo entry: {:?}", t.app.engine.undo_history());
    let s = t.app.engine.session();
    assert!(s.bus(bus).is_none(), "its bus went with it");
    assert!(s.tracks.iter().all(|x| x.folder.is_none() && x.mixer.output == soundcraft_model::Route::Main));
    assert_eq!(s.edit.selected_tracks, vec![t.a, t.b], "the former members are selected");
    assert_eq!(t.strips(), ["A", "B", "C"]);
    t.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(t.order(), ["Group 1", "A", "B", "C"], "undo brings the group back");
}

#[test]
fn group_and_ungroup_from_the_header_menu() {
    let mut t = T::new();
    t.right_click(t.header(t.b, 0.5));
    let item = t.app.arrange.menu.iter().find(|m| m.label == "Group Tracks").cloned().expect("Group Tracks in the header menu");
    t.click(mid(item.rect), json!({}));
    let g = t.groups();
    assert_eq!(g.len(), 1);
    assert_eq!(t.folder_of(t.b), Some(g[0]), "the right-clicked track, alone (it was not selected)");
    assert_eq!(t.folder_of(t.a), None);
    t.right_click(t.header(t.b, 0.5));
    let item = t.app.arrange.menu.iter().find(|m| m.label == "Ungroup").cloned().expect("Ungroup in a member's menu");
    t.click(mid(item.rect), json!({}));
    assert!(t.groups().is_empty());
    assert_eq!(t.order(), ["A", "B", "C"]);
}

#[test]
fn alt_arrows_move_the_selected_tracks() {
    let mut t = T::new();
    t.click(t.header(t.a, 0.5), json!({}));
    let u0 = t.undo_len();
    t.key("ArrowDown", json!({"alt": true}));
    assert_eq!(t.order(), ["B", "A", "C"]);
    assert_eq!(t.strips(), ["B", "A", "C"]);
    assert_eq!(t.undo_len(), u0 + 1);
    t.key("ArrowDown", json!({"alt": true}));
    t.key("ArrowDown", json!({"alt": true}));
    assert_eq!(t.order(), ["B", "C", "A"], "stops at the bottom");
    assert_eq!(t.undo_len(), u0 + 2, "no entry when nothing moved");
    t.key("ArrowUp", json!({"alt": true}));
    assert_eq!(t.order(), ["B", "A", "C"]);
    // A group moves as a block, and a track passes a whole group.
    t.click(t.header(t.b, 0.5), json!({}));
    t.click(t.header(t.a, 0.5), json!({"shift": true}));
    t.key("G", json!({"cmd": true}));
    assert_eq!(t.order(), ["Group 1", "B", "A", "C"]);
    t.click(t.header(t.c, 0.5), json!({}));
    t.key("ArrowUp", json!({"alt": true}));
    assert_eq!(t.order(), ["C", "Group 1", "B", "A"]);
    // Inside a group, a member moves among the members.
    t.click(t.header(t.a, 0.5), json!({}));
    t.key("ArrowUp", json!({"alt": true}));
    assert_eq!(t.order(), ["C", "Group 1", "A", "B"]);
    t.key("ArrowUp", json!({"alt": true}));
    assert_eq!(t.order(), ["C", "Group 1", "A", "B"], "a member stays in its group");
}

#[test]
fn header_clicks_select_ranges_toggle_and_focus_a_group() {
    let mut t = T::new();
    let g = t.group_ab();
    let u0 = t.undo_len();
    // A plain click on the group's header selects and focuses it.
    t.click(t.header(t.c, 0.5), json!({}));
    t.click(t.header(g, 0.5), json!({}));
    assert_eq!(t.app.engine.session().edit.selected_tracks, vec![g]);
    assert_eq!(t.app.mixer.devices.track, Some(g.0));
    // Shift-click ranges over the members.
    t.click(t.header(t.c, 0.5), json!({"shift": true}));
    assert_eq!(t.app.engine.session().edit.selected_tracks, vec![g, t.a, t.b, t.c]);
    // Ctrl-click takes one out and puts it back.
    t.click(t.header(t.a, 0.5), json!({"cmd": true}));
    assert_eq!(t.app.engine.session().edit.selected_tracks, vec![g, t.b, t.c]);
    t.click(t.header(t.a, 0.5), json!({"cmd": true}));
    assert_eq!(t.app.engine.session().edit.selected_tracks, vec![g, t.a, t.b, t.c]);
    assert_eq!(t.undo_len(), u0, "selecting is never an undo entry");
}

#[test]
fn the_track_menu_has_group_and_ungroup() {
    let mut t = T::new();
    t.click(t.header(t.a, 0.5), json!({}));
    t.app.group_selected().unwrap();
    t.frames(2);
    assert_eq!(t.groups().len(), 1);
    assert_eq!(t.app.ungroup_targets(), t.groups(), "the focused group is what Ungroup takes");
    t.app.ungroup_selected().unwrap();
    assert!(t.groups().is_empty());
    assert!(t.app.ungroup_selected().is_err(), "nothing to ungroup");
}

#[test]
fn the_track_menu_groups_and_ungroups() {
    let mut t = T::new();
    t.click(t.header(t.a, 0.5), json!({}));
    t.click(t.header(t.c, 0.5), json!({"shift": true}));
    let open = |t: &mut T| {
        let m = t.app.arrange.bar_menu.iter().find(|m| m.label == "Track").cloned().expect("the Track menu");
        t.click(mid(m.rect), json!({}));
    };
    open(&mut t);
    let item = t.app.arrange.bar_menu.iter().find(|m| m.label == "Group Tracks").cloned().expect("Group Tracks in the Track menu");
    t.click(mid(item.rect), json!({}));
    let g = t.groups();
    assert_eq!(g.len(), 1);
    assert_eq!(t.order(), ["Group 1", "A", "B", "C"], "the whole selection");
    open(&mut t);
    let item = t.app.arrange.bar_menu.iter().find(|m| m.label == "Ungroup").cloned().expect("Ungroup in the Track menu");
    t.click(mid(item.rect), json!({}));
    assert!(t.groups().is_empty());
    assert_eq!(t.order(), ["A", "B", "C"]);
}
