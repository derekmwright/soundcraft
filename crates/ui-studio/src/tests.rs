use crate::{Services, StudioApp};
use serde_json::{Value, json};
use soundcraft_model::{ClipId, TrackId};

fn app() -> StudioApp {
    StudioApp::new(soundcraft_engine::Engine::default(), None, Services::default())
}

/// An instrument track with a one-bar MIDI clip at bar 1; returns (track, clip, bar length).
fn track_with_clip(app: &mut StudioApp) -> (TrackId, ClipId, i64) {
    let r = app.run("track.new", json!({"kind": "instrument"})).unwrap();
    let t = TrackId(r["tracks"][0].as_u64().unwrap());
    let c = app.run("midi.new_clip", json!({"tracks": [t.0], "start": 0, "end": 0})).unwrap();
    let bar = c["end"].as_i64().unwrap() - c["start"].as_i64().unwrap();
    (t, ClipId(c["clip"].as_u64().unwrap()), bar)
}

fn clips_on(app: &StudioApp, t: TrackId) -> Vec<(u64, i64, i64)> {
    app.engine.session().track(t).unwrap().clips().iter().map(|c| (c.id.0, c.start, c.end())).collect()
}

#[test]
fn the_cursor_is_not_the_selection() {
    let mut app = app();
    let (t, clip, bar) = track_with_clip(&mut app);
    app.click_empty(Some(t), 3 * bar);
    app.click_clip(clip, false);
    let s = app.engine.session();
    assert_eq!(s.edit.selected_clips, vec![clip]);
    assert_eq!((s.edit.selection.start, s.edit.selection.end), (0, bar));
    assert_eq!(app.cursor(), 3 * bar, "selecting a clip leaves the cursor where it was");
    assert_eq!(app.cursor_track(), Some(t));
    app.click_ruler(5 * bar);
    assert_eq!(app.cursor(), 5 * bar);
    assert_eq!(app.engine.session().edit.selected_clips, vec![clip], "the ruler moves the cursor, not the selection");
}

#[test]
fn paste_goes_to_the_cursor_and_never_onto_the_copied_clip() {
    let mut app = app();
    let (t, clip, bar) = track_with_clip(&mut app);
    app.click_empty(Some(t), 0);
    app.click_clip(clip, false);
    app.copy_selected(false).unwrap();
    // The cursor is on the original: refused, nothing replaced.
    assert!(app.paste_at_cursor().is_err());
    assert_eq!(clips_on(&app, t).len(), 1);
    assert!(app.status.contains("replace"), "{}", app.status);
    app.click_empty(Some(t), 4 * bar);
    app.paste_at_cursor().unwrap();
    let clips = clips_on(&app, t);
    assert_eq!(clips.len(), 2, "{clips:?}");
    assert!(clips.iter().any(|c| c.0 == clip.0 && c.1 == 0), "the original stays");
    assert!(clips.iter().any(|c| c.1 == 4 * bar && c.2 == 5 * bar), "the copy is at the cursor: {clips:?}");
}

#[test]
fn alt_drag_copies_and_keeps_the_clipboard() {
    let mut app = app();
    let (t, clip, bar) = track_with_clip(&mut app);
    let before = app.engine.clipboard.clone();
    app.copy_clip_to(clip, 2 * bar, t).unwrap();
    let clips = clips_on(&app, t);
    assert_eq!(clips.len(), 2);
    assert!(clips.iter().any(|c| c.0 == clip.0 && c.1 == 0));
    assert!(clips.iter().any(|c| c.0 != clip.0 && c.1 == 2 * bar));
    assert_eq!(app.engine.clipboard, before);
    let copy = clips.iter().find(|c| c.0 != clip.0).unwrap().0;
    assert_eq!(app.engine.session().edit.selected_clips, vec![ClipId(copy)], "the copy is selected");
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(clips_on(&app, t).len(), 1, "one undo step");
}

#[test]
fn move_trim_split_delete_are_commands() {
    let mut app = app();
    let (t, clip, bar) = track_with_clip(&mut app);
    let r = app.run("track.new", json!({"kind": "instrument"})).unwrap();
    let t2 = TrackId(r["tracks"][0].as_u64().unwrap());
    app.move_clips(&[clip], bar, Some(t2)).unwrap();
    assert!(clips_on(&app, t).is_empty());
    assert_eq!(clips_on(&app, t2), vec![(clip.0, bar, 2 * bar)]);
    app.trim_clip(clip, false, bar + bar / 2).unwrap();
    assert_eq!(clips_on(&app, t2), vec![(clip.0, bar, bar + bar / 2)]);
    app.click_clip(clip, false);
    app.set_cursor(bar + bar / 4, Some(t2));
    app.split_at_cursor().unwrap();
    let parts = clips_on(&app, t2);
    assert_eq!(parts.len(), 2, "{parts:?}");
    let first = parts.iter().find(|c| c.1 == bar).unwrap().0;
    app.click_clip(ClipId(first), false);
    app.delete_selected().unwrap();
    assert_eq!(clips_on(&app, t2).iter().map(|c| (c.1, c.2)).collect::<Vec<_>>(), vec![(bar + bar / 4, bar + bar / 2)]);
}

#[test]
fn play_starts_at_the_cursor_and_stop_returns_there() {
    let mut app = app();
    app.set_cursor(48_000, None);
    app.run("transport.play", json!({})).unwrap();
    app.handle_transport(0.25);
    assert!(app.is_playing());
    assert_eq!(app.position(), 48_000 + 12_000);
    app.run("transport.toggle", json!({})).unwrap();
    app.handle_transport(0.0);
    assert!(!app.is_playing());
    assert_eq!(app.position(), 48_000);
    app.run("transport.rtz", json!({})).unwrap();
    app.handle_transport(0.0);
    assert_eq!(app.cursor(), 0);
}

#[test]
fn new_session_stops_playback() {
    let mut app = StudioApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.run("transport.play", json!({"from": 48_000})).unwrap();
    app.handle_transport(0.25);
    assert!(app.is_playing());
    app.run("session.new", json!({})).unwrap();
    app.handle_transport(0.25);
    assert!(!app.is_playing());
    app.handle_transport(0.25);
    assert_eq!(app.engine.transport.position, app.cursor(), "the old clock stays stopped");
}

#[test]
fn shuffle_sessions_open_in_slip() {
    let mut e = soundcraft_engine::Engine::default();
    e.execute("edit.mode", &json!({"mode": "shuffle"})).unwrap();
    let app = StudioApp::new(e, None, Services::default());
    assert_eq!(app.engine.session().edit.edit_mode, soundcraft_model::EditMode::Slip);
}

#[test]
fn ui_set_lists_editors_without_a_player() {
    let mut app = app();
    let v = app.run("ui.set", json!({"plugin_windows": [[3, usize::MAX]]})).unwrap();
    assert_eq!(v["ui"]["plugin_windows"][0][0], json!(3));
    assert!(app.run("ui.set", json!({"plugin_dirs": {"vst3": ["x"]}})).is_err());
}

/// Run frames of the real UI, feeding `app.synthetic` like the control channel does.
fn frames(app: &mut StudioApp, ctx: &egui::Context, n: usize, t0: &mut f64) {
    for _ in 0..n {
        *t0 += 1.0 / 60.0;
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0))),
            time: Some(*t0),
            ..Default::default()
        };
        app.raw_input_hook(&mut raw);
        let mut out = ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }
}

fn drag(app: &mut StudioApp, from: (f32, f32), to: (f32, f32), mods: Value) {
    let mut p = json!({"x": from.0, "y": from.1, "to_x": to.0, "to_y": to.1});
    if let (Some(o), Some(m)) = (p.as_object_mut(), mods.as_object()) {
        o.extend(m.clone());
    }
    let (req, _rx) = crate::ControlRequest::new("ui.drag", p);
    let ctx = egui::Context::default();
    let _ = crate::control::handle_for_test(app, &ctx, &req);
}

#[test]
fn dragging_a_clip_in_the_window_moves_copies_and_constrains_it() {
    let mut app = app();
    let (t, clip, bar) = track_with_clip(&mut app);
    let r = app.run("track.new", json!({"kind": "instrument"})).unwrap();
    let t2 = TrackId(r["tracks"][0].as_u64().unwrap());
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    let layout = app.arrange.layout.clone();
    let rect = layout.clips.iter().find(|c| c.id == clip.0).unwrap().rect;
    let lane2 = layout.tracks.iter().find(|r| r.id == t2.0).unwrap().lane;
    let px_per_bar = bar as f64 / layout.samples_per_px;
    let grab = (rect[0] + 20.0, (rect[1] + rect[3]) / 2.0);
    // Plain drag: one bar later, onto the second track.
    let to = (grab.0 + px_per_bar as f32, (lane2[1] + lane2[3]) / 2.0);
    drag(&mut app, grab, to, json!({}));
    frames(&mut app, &ctx, 20, &mut time);
    let moved = clips_on(&app, t2);
    assert_eq!(moved.len(), 1, "moved to the second track: {moved:?}");
    assert!((moved[0].1 - bar).abs() <= layout.samples_per_px.ceil() as i64, "{moved:?}");
    // Alt-drag it back up: a copy on the first track, the original stays.
    let rect = app.arrange.layout.clips.iter().find(|c| c.id == clip.0).unwrap().rect;
    let lane1 = app.arrange.layout.tracks.iter().find(|r| r.id == t.0).unwrap().lane;
    let grab = (rect[0] + 20.0, (rect[1] + rect[3]) / 2.0);
    drag(&mut app, grab, (grab.0, (lane1[1] + lane1[3]) / 2.0), json!({"alt": true}));
    frames(&mut app, &ctx, 20, &mut time);
    assert_eq!(clips_on(&app, t2).len(), 1);
    assert_eq!(clips_on(&app, t).len(), 1, "the Alt-drag copied");
    // Ctrl-drag toward the first track: stays on its own track, moves in time.
    let start = clips_on(&app, t2)[0].1;
    drag(&mut app, grab, (grab.0 + px_per_bar as f32, (lane1[1] + lane1[3]) / 2.0), json!({"ctrl": true}));
    frames(&mut app, &ctx, 20, &mut time);
    let after = clips_on(&app, t2);
    assert_eq!(after.len(), 1, "constrained to its track: {after:?}");
    assert!(after[0].1 > start);
    assert_eq!(clips_on(&app, t).len(), 1);
}
