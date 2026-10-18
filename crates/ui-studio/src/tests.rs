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
fn ui_set_lists_editors_without_a_player() {
    let mut app = app();
    let v = app.run("ui.set", json!({"plugin_windows": [[3, usize::MAX]]})).unwrap();
    assert_eq!(v["ui"]["plugin_windows"][0][0], json!(3));
    assert!(app.run("ui.set", json!({"plugin_dirs": {"vst3": ["x"]}})).is_err());
}

#[test]
fn inspect_reports_the_plugin_scan_without_starting_one() {
    // Tests never scan: nothing starts one, so the state is "idle" with no counts.
    let scan = crate::plugin_scan::inspect();
    assert_eq!(scan["state"], json!("idle"));
    assert!(scan["vst3"].is_null() && scan["clap"].is_null());
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

#[test]
fn setting_the_cursor_does_not_dirty_the_document() {
    let mut app = app();
    let (t, _clip, bar) = track_with_clip(&mut app);
    let path = std::env::temp_dir().join(format!("transit-cursor-{}.sc", std::process::id()));
    app.run("session.save_as", json!({"path": path.to_string_lossy(), "replace": true})).unwrap();
    assert!(!app.engine.is_dirty());
    app.set_cursor(bar, Some(t));
    assert_eq!(app.cursor(), bar);
    assert!(!app.engine.is_dirty(), "the cursor is view state");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_shuffle_document_keeps_its_mode_and_stays_clean() {
    let mut e = soundcraft_engine::Engine::default();
    e.execute("edit.mode", &json!({"mode": "shuffle"})).unwrap();
    let path = std::env::temp_dir().join(format!("transit-shuffle-{}.sc", std::process::id()));
    e.execute("session.save_as", &json!({"path": path.to_string_lossy(), "replace": true})).unwrap();
    let mut app = StudioApp::new(e, None, Services::default());
    assert_eq!(app.engine.session().edit.edit_mode, soundcraft_model::EditMode::Shuffle);
    assert!(!app.engine.is_dirty());
    app.run("session.open", json!({"path": path.to_string_lossy()})).unwrap();
    app.handle_transport(0.0);
    assert_eq!(app.engine.session().edit.edit_mode, soundcraft_model::EditMode::Shuffle);
    assert!(!app.engine.is_dirty());
    // Edits still behave as Slip, and the mode is put back afterwards.
    let (t, clip, bar) = track_with_clip(&mut app);
    app.run("edit.mode", json!({"mode": "shuffle"})).unwrap();
    app.move_clips(&[clip], bar, None).unwrap();
    assert_eq!(clips_on(&app, t), vec![(clip.0, bar, 2 * bar)]);
    assert_eq!(app.engine.session().edit.edit_mode, soundcraft_model::EditMode::Shuffle);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn alt_drag_onto_its_own_range_is_refused() {
    let mut app = app();
    let (t, clip, _bar) = track_with_clip(&mut app);
    let before = clips_on(&app, t);
    assert!(app.copy_clip_to(clip, 10_000, t).is_err());
    assert_eq!(clips_on(&app, t), before, "the original is not truncated");
    assert!(app.status.contains("Copy would land"));
}

// ---- Slice 2: mixer and device chain ----

fn undo_len(app: &mut StudioApp) -> usize {
    app.engine.execute("engine.history", &json!({})).unwrap()["undo"].as_array().unwrap().len()
}

fn click(app: &mut StudioApp, at: (f32, f32)) {
    let (req, _rx) = crate::ControlRequest::new("ui.click", json!({"x": at.0, "y": at.1}));
    let ctx = egui::Context::default();
    let _ = crate::control::handle_for_test(app, &ctx, &req);
}

fn mid(r: [f32; 4]) -> (f32, f32) {
    ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
}

/// An app with the mixer pane open and a stereo audio track; returns (app, ctx, time, track).
fn mixer_app() -> (StudioApp, egui::Context, f64, TrackId) {
    let mut app = app();
    let r = app.run("track.new", json!({"kind": "audio", "format": "stereo", "name": "Gtr"})).unwrap();
    let t = TrackId(r["tracks"][0].as_u64().unwrap());
    app.ui.show_mixer = true;
    app.ui.show_devices = true;
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    (app, ctx, time, t)
}

fn strip(app: &StudioApp, t: TrackId) -> crate::mixer::StripLayout {
    app.mixer.layout.strips.iter().find(|s| s.track == t.0).cloned().unwrap()
}

#[test]
fn fader_and_balance_laws_round_trip() {
    use crate::mixer::{balance_of, db_to_k, k_to_db, pans_for};
    for db in [-60.0f32, -24.0, -6.0, 0.0, 6.0, 12.0] {
        assert!((k_to_db(db_to_k(db)) - db).abs() < 0.01, "{db}");
    }
    assert_eq!(db_to_k(-144.0), 0.0);
    assert_eq!(k_to_db(0.0), -144.0);
    assert!((db_to_k(12.0) - 1.0).abs() < 1e-6);
    assert_eq!(pans_for(0.0, 2), vec![-1.0, 1.0], "center keeps the stereo image");
    assert_eq!(pans_for(-1.0, 2), vec![-1.0, -1.0]);
    assert_eq!(pans_for(1.0, 2), vec![1.0, 1.0]);
    assert_eq!(pans_for(0.5, 1), vec![0.5]);
    for b in [-1.0f32, -0.3, 0.0, 0.4, 1.0] {
        assert!((balance_of(&pans_for(b, 2)) - b).abs() < 1e-6);
    }
}

#[test]
fn a_fader_drag_is_one_undo_step() {
    let (mut app, ctx, mut time, t) = mixer_app();
    let f = strip(&app, t).fader;
    let k0 = crate::mixer::db_to_k(0.0);
    let cap_y = f[3] - k0 * (f[3] - f[1]);
    let x = (f[0] + f[2]) / 2.0;
    let before = undo_len(&mut app);
    drag(&mut app, (x, cap_y), (x, cap_y + 40.0), json!({}));
    frames(&mut app, &ctx, 30, &mut time);
    let db = app.engine.session().track(t).unwrap().mixer.volume_db;
    assert!(db < -3.0, "the fader went down: {db}");
    assert_eq!(undo_len(&mut app), before + 1, "one gesture, one undo step");
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(app.engine.session().track(t).unwrap().mixer.volume_db, 0.0);
}

#[test]
fn the_pan_knob_is_a_balance_control() {
    let (mut app, ctx, mut time, t) = mixer_app();
    let k = mid(strip(&app, t).knob);
    let before = undo_len(&mut app);
    // 50 points up: half way to the right.
    drag(&mut app, k, (k.0, k.1 - 50.0), json!({}));
    frames(&mut app, &ctx, 30, &mut time);
    let s = strip(&app, t);
    assert!((s.balance - 0.5).abs() < 0.03, "{}", s.balance);
    let pan = app.engine.session().track(t).unwrap().mixer.pan.clone();
    let want = crate::mixer::pans_for(s.balance, 2);
    assert!((pan[0] - want[0]).abs() < 1e-5 && (pan[1] - want[1]).abs() < 1e-5, "{pan:?} vs {want:?}");
    assert_eq!(undo_len(&mut app), before + 1, "both panners, one undo step");
}

#[test]
fn plus_inserts_a_built_in_device_and_bypass_and_remove_round_trip() {
    let (mut app, ctx, mut time, t) = mixer_app();
    app.focus(t);
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(app.mixer.devices.track, Some(t.0));
    assert!(app.mixer.devices.cards.is_empty());
    let at = mid(app.mixer.devices.add);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.mixer.devices.menu.iter().find(|m| m.value == "gain").cloned().expect("the + menu lists the built-in Gain");
    assert!(app.mixer.devices.menu.iter().all(|m| !m.value.contains(':')), "no hosted plug-ins in a hermetic test");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    let ins = app.engine.session().track(t).unwrap().mixer.inserts.clone();
    assert_eq!(ins.iter().flatten().map(|i| i.plugin.as_str()).collect::<Vec<_>>(), vec!["gain"]);
    let card = app.mixer.devices.cards.first().cloned().expect("a device card");
    assert_eq!((card.slot, card.plugin.as_str()), (Some(0), "gain"));
    assert!(!card.editor_enabled, "no editor without a player");
    click(&mut app, mid(card.bypass_button));
    frames(&mut app, &ctx, 6, &mut time);
    assert!(app.engine.session().track(t).unwrap().mixer.inserts[0].as_ref().unwrap().bypass);
    assert!(app.mixer.devices.cards[0].bypass);
    click(&mut app, mid(card.bypass_button));
    frames(&mut app, &ctx, 6, &mut time);
    assert!(!app.engine.session().track(t).unwrap().mixer.inserts[0].as_ref().unwrap().bypass);
    click(&mut app, mid(card.remove_button));
    frames(&mut app, &ctx, 6, &mut time);
    assert!(app.engine.session().track(t).unwrap().mixer.inserts.iter().all(Option::is_none));
    assert!(app.mixer.devices.cards.is_empty());
}

#[test]
fn the_instrument_is_the_first_device() {
    let mut app = app();
    let r = app.run("track.new", json!({"kind": "instrument"})).unwrap();
    let t = TrackId(r["tracks"][0].as_u64().unwrap());
    app.add_device(t, "gain").unwrap();
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    let cards = &app.mixer.devices.cards;
    assert_eq!(cards.iter().map(|c| c.slot).collect::<Vec<_>>(), vec![None, Some(0)]);
    assert_eq!(cards[0].plugin, "subtractive_synth");
}

#[test]
fn the_output_dropdown_runs_track_output() {
    let (mut app, ctx, mut time, t) = mixer_app();
    let r = app.run("track.new", json!({"kind": "audio", "name": "Vox"})).unwrap();
    let t2 = TrackId(r["tracks"][0].as_u64().unwrap());
    let fx = app.add_fx_channel().unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let at = mid(strip(&app, t).output);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.mixer.layout.menu.iter().find(|m| m.label == "FX 1").cloned().expect("the FX channel is offered by name");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    app.run("track.output", json!({"tracks": [t2.0], "output": item.value})).unwrap();
    let s = app.engine.session();
    let (a, b) = (s.track(t).unwrap().mixer.output.clone(), s.track(t2).unwrap().mixer.output.clone());
    assert_eq!(a, b, "the dropdown and the command agree");
    assert_eq!(s.track(fx).unwrap().mixer.input, a, "the FX channel listens to that bus");
    assert_eq!(strip(&app, t).output_label, "FX 1");
}

#[test]
fn add_fx_channel_is_one_undo_step() {
    let mut app = app();
    let r = app.run("track.new", json!({"kind": "audio", "name": "Gtr"})).unwrap();
    let t = TrackId(r["tracks"][0].as_u64().unwrap());
    app.focus(t);
    let before = undo_len(&mut app);
    let tracks = app.engine.session().tracks.len();
    let fx = app.add_fx_channel().unwrap();
    let s = app.engine.session();
    let aux = s.track(fx).unwrap();
    assert_eq!(aux.kind, soundcraft_model::TrackKind::Aux);
    let bus = s.bus_by_name("FX 1").unwrap().id;
    assert_eq!(aux.mixer.input, soundcraft_model::Route::Bus(bus));
    let send = s.track(t).unwrap().mixer.sends.iter().flatten().next().unwrap();
    assert_eq!(send.target, soundcraft_model::Route::Bus(bus));
    assert_eq!(undo_len(&mut app), before + 1);
    app.run("edit.undo", json!({})).unwrap();
    let s = app.engine.session();
    assert_eq!(s.tracks.len(), tracks);
    assert!(s.bus_by_name("FX 1").is_none() && s.track(t).unwrap().mixer.sends.iter().all(Option::is_none));
    // A second one gets the next name and its own undo step.
    app.run("edit.redo", json!({})).unwrap();
    app.add_fx_channel().unwrap();
    assert!(app.engine.session().bus_by_name("FX 2").is_some());
    assert_eq!(undo_len(&mut app), before + 2);
}

#[test]
fn focus_follows_both_views_and_views_never_dirty_the_document() {
    let (mut app, ctx, mut time, t) = mixer_app();
    let r = app.run("track.new", json!({"kind": "instrument", "name": "Keys"})).unwrap();
    let t2 = TrackId(r["tracks"][0].as_u64().unwrap());
    let path = std::env::temp_dir().join(format!("transit-mixer-{}.sc", std::process::id()));
    app.run("session.save_as", json!({"path": path.to_string_lossy(), "replace": true})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    // Mixer to arrangement: click the first strip's name.
    let at = mid(strip(&app, t).name_rect);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(app.focus_track(), Some(t));
    assert_eq!(app.engine.session().edit.selected_tracks, vec![t], "the arrangement's selected track");
    assert_eq!(app.mixer.devices.track, Some(t.0));
    // Arrangement to mixer: click the second track's header.
    let hdr = app.arrange.layout.tracks.iter().find(|r| r.id == t2.0).unwrap().header;
    click(&mut app, (hdr[0] + 30.0, (hdr[1] + hdr[3]) / 2.0));
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(app.focus_track(), Some(t2));
    assert_eq!(app.mixer.devices.track, Some(t2.0));
    // View toggles: F3 twice, Ctrl+Alt+M and Ctrl+Alt+L are view state only.
    for (key, mods) in [("F3", json!({})), ("F3", json!({})), ("M", json!({"cmd": true, "alt": true})), ("L", json!({"cmd": true, "alt": true}))] {
        let mut p = json!({"key": key});
        if let (Some(o), Some(m)) = (p.as_object_mut(), mods.as_object()) {
            o.extend(m.clone());
        }
        let (req, _rx) = crate::ControlRequest::new("ui.key", p);
        let _ = crate::control::handle_for_test(&mut app, &ctx, &req);
        frames(&mut app, &ctx, 4, &mut time);
    }
    assert!(!app.ui.show_mixer && !app.ui.show_devices && !app.ui.mixer_full, "{:?}", (app.ui.show_mixer, app.ui.show_devices, app.ui.mixer_full));
    assert!(!app.engine.is_dirty(), "focus and panes are view state");
    let _ = std::fs::remove_file(&path);
}

// --- Slice 2b: instrument menu, sends, routing loops, review follow-ups ---

fn new_track(app: &mut StudioApp, p: Value) -> TrackId {
    TrackId(app.run("track.new", p).unwrap()["tracks"][0].as_u64().unwrap())
}

fn right_click(app: &mut StudioApp, at: (f32, f32)) {
    let (req, _rx) = crate::ControlRequest::new("ui.click", json!({"x": at.0, "y": at.1, "button": "right"}));
    let ctx = egui::Context::default();
    let _ = crate::control::handle_for_test(app, &ctx, &req);
}

fn hover(app: &mut StudioApp, at: (f32, f32)) {
    let (req, _rx) = crate::ControlRequest::new("ui.move", json!({"x": at.0, "y": at.1}));
    let ctx = egui::Context::default();
    let _ = crate::control::handle_for_test(app, &ctx, &req);
}

fn instrument_of(app: &StudioApp, t: TrackId) -> Option<soundcraft_model::Insert> {
    app.engine.session().track(t).unwrap().instrument.clone()
}

#[test]
fn the_instrument_menu_runs_mix_instrument_and_a_replace_is_one_undo_step() {
    let mut app = app();
    let t = new_track(&mut app, json!({"kind": "instrument", "name": "Keys"}));
    let t2 = new_track(&mut app, json!({"kind": "instrument", "name": "Direct"}));
    app.ui.show_devices = true;
    app.focus(t);
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    let card = app.mixer.devices.cards.first().cloned().unwrap();
    assert_eq!((card.slot, card.plugin.as_str()), (None, "subtractive_synth"));
    let before = undo_len(&mut app);
    click(&mut app, mid(card.pick_button));
    frames(&mut app, &ctx, 6, &mut time);
    let menu = app.mixer.devices.menu.clone();
    assert!(menu.iter().any(|m| m.value == "sampler"), "built-in instruments are listed: {menu:?}");
    assert!(menu.iter().all(|m| m.value != "gain"), "no effects in the instrument menu");
    assert!(menu.iter().all(|m| !m.value.contains(':')), "no hosted plug-ins in a hermetic test");
    let item = menu.iter().find(|m| m.value == "drum_synth").cloned().unwrap();
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(instrument_of(&app, t).unwrap().plugin, "drum_synth");
    assert_eq!(undo_len(&mut app), before + 1, "a replace is one undo step");
    app.run("mix.instrument", json!({"track": t2.0, "plugin": "drum_synth"})).unwrap();
    assert_eq!(instrument_of(&app, t), instrument_of(&app, t2), "the menu and mix.instrument give the same instrument");
    assert_eq!(app.mixer.devices.cards[0].plugin, "drum_synth");
    // Picking the instrument it already has changes nothing.
    let n = undo_len(&mut app);
    app.set_instrument(t, "drum_synth").unwrap();
    assert_eq!(undo_len(&mut app), n);
    app.run("edit.undo", json!({})).unwrap();
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(instrument_of(&app, t).unwrap().plugin, "subtractive_synth");
}

#[test]
fn an_instrument_track_without_an_instrument_offers_choose() {
    let mut app = app();
    let t = new_track(&mut app, json!({"kind": "instrument"}));
    // A session saved without an instrument (test setup; the UI itself only executes commands).
    app.engine.session_mut().track_mut(t).unwrap().instrument = None;
    app.ui.show_devices = true;
    app.focus(t);
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    let card = app.mixer.devices.cards.first().cloned().unwrap();
    assert_eq!((card.slot, card.plugin.as_str(), card.name.as_str()), (None, "", "No instrument"));
    assert_ne!(card.pick_button, [0.0; 4], "a Choose… button");
    click(&mut app, mid(card.pick_button));
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.mixer.devices.menu.iter().find(|m| m.value == "sampler").cloned().unwrap();
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(instrument_of(&app, t).unwrap().plugin, "sampler");
}

#[test]
fn the_track_menu_has_an_instrument_submenu() {
    let mut app = app();
    let t = new_track(&mut app, json!({"kind": "instrument"}));
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    let hdr = app.arrange.layout.tracks.iter().find(|r| r.id == t.0).unwrap().header;
    right_click(&mut app, (hdr[0] + 30.0, (hdr[1] + hdr[3]) / 2.0));
    frames(&mut app, &ctx, 6, &mut time);
    let sub = app.arrange.menu.iter().find(|m| m.label == "Instrument").cloned().expect("Instrument in the track menu");
    hover(&mut app, mid(sub.rect));
    frames(&mut app, &ctx, 10, &mut time);
    let item = app.arrange.menu.iter().find(|m| m.value == "drum_synth").cloned().expect("the submenu lists the instruments");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(instrument_of(&app, t).unwrap().plugin, "drum_synth");
}

#[test]
fn a_long_instrument_list_filters_and_groups_by_vendor() {
    use crate::devices::{HostedEntry, InstrumentLists, instrument_menu};
    let hosted =
        |v: &'static str, n: usize| (0..n).map(move |i| HostedEntry { id: format!("vst3:{v}{i}"), name: format!("{v} Synth {i}"), vendor: v.into() });
    let lists =
        InstrumentLists { builtin: Vec::new(), vst3: Some(hosted("Arturia", 10).chain(hosted("Native", 10)).collect()), clap: Some(Vec::new()) };
    let ctx = egui::Context::default();
    let mut items = Vec::new();
    let mut filter = "arturia".to_string();
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
        instrument_menu(ui, &lists, None, &mut filter, &mut Default::default(), &mut items);
    });
    out.textures_delta.clear();
    assert_eq!(items.len(), 10, "{items:?}");
    assert!(items.iter().all(|m| m.value.starts_with("vst3:Arturia")));
}

#[test]
fn a_send_row_drag_is_one_undo_step() {
    let (mut app, ctx, mut time, t) = mixer_app();
    app.new_fx_channel(Some(t), 0.0).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let row = strip(&app, t).sends.first().cloned().expect("a send row");
    assert_eq!((row.slot, row.target.as_str(), row.level_db), (0, "FX 1", 0.0));
    let before = undo_len(&mut app);
    let (x, y) = mid(row.level_rect);
    drag(&mut app, (x, y), (x, y + 30.0), json!({}));
    frames(&mut app, &ctx, 30, &mut time);
    let level = app.engine.session().track(t).unwrap().mixer.sends[0].as_ref().unwrap().level_db;
    assert!(level < -3.0, "the drag lowered the send: {level}");
    assert_eq!(undo_len(&mut app), before + 1, "one gesture, one undo step");
    assert_eq!(strip(&app, t).sends[0].level_db, level, "the row shows mix.send's level");
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(app.engine.session().track(t).unwrap().mixer.sends[0].as_ref().unwrap().level_db, 0.0);
}

#[test]
fn new_fx_channel_from_a_strip_is_one_undo_step_at_0_db() {
    let (mut app, ctx, mut time, t) = mixer_app();
    let tracks = app.engine.session().tracks.len();
    let before = undo_len(&mut app);
    let at = mid(strip(&app, t).add_send);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.mixer.layout.menu.iter().find(|m| m.value == crate::mixer::NEW_FX).cloned().expect("New FX Channel… in + Send");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    let s = app.engine.session();
    let bus = s.bus_by_name("FX 1").expect("a bus").id;
    let fx = s.tracks.iter().find(|x| x.name == "FX 1").expect("an FX channel");
    assert_eq!((fx.kind, &fx.mixer.input), (soundcraft_model::TrackKind::Aux, &soundcraft_model::Route::Bus(bus)));
    let send = s.track(t).unwrap().mixer.sends[0].clone().unwrap();
    assert_eq!((send.target, send.level_db), (soundcraft_model::Route::Bus(bus), 0.0));
    let fx_id = TrackId(fx.id.0);
    assert_eq!(undo_len(&mut app), before + 1);
    let fs = strip(&app, fx_id);
    assert!(fs.fx, "the FX channel's strip is marked");
    assert_eq!(fs.input_label, "Bus: FX 1", "its input shows the bus");
    app.run("edit.undo", json!({})).unwrap();
    let s = app.engine.session();
    assert_eq!(s.tracks.len(), tracks);
    assert!(s.bus_by_name("FX 1").is_none() && s.track(t).unwrap().mixer.sends.iter().all(Option::is_none));
}

#[test]
fn no_free_send_slot_leaves_no_bus_or_channel_behind() {
    let mut app = app();
    let t = new_track(&mut app, json!({"kind": "audio", "name": "Gtr"}));
    for i in 0..10 {
        app.run("mix.send", json!({"track": t.0, "bus": format!("B{i}")})).unwrap();
    }
    let (busses, tracks, undo) = (app.engine.session().busses.len(), app.engine.session().tracks.len(), undo_len(&mut app));
    assert!(app.new_fx_channel(Some(t), 0.0).is_err());
    assert!(app.status.contains("sends"), "{}", app.status);
    let s = app.engine.session();
    assert_eq!((s.busses.len(), s.tracks.len()), (busses, tracks));
    assert_eq!(undo_len(&mut app), undo);
    // A step that fails after the bus is made (no such track to send from) is taken back.
    assert!(app.new_fx_channel(Some(TrackId(999_999)), 0.0).is_err());
    let s = app.engine.session();
    assert_eq!((s.busses.len(), s.tracks.len()), (busses, tracks), "rolled back");
    assert_eq!(undo_len(&mut app), undo);
}

#[test]
fn picking_the_default_input_changes_nothing() {
    let (mut app, ctx, mut time, t) = mixer_app();
    assert_eq!(strip(&app, t).input_label, "In 1-2");
    let before = undo_len(&mut app);
    let at = mid(strip(&app, t).input);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.mixer.layout.menu.iter().find(|m| m.label == "In 1-2").cloned().unwrap();
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(undo_len(&mut app), before, "no document edit");
    assert_eq!(app.engine.session().track(t).unwrap().mixer.input, soundcraft_model::Route::None);
    // Another input is a real change.
    app.set_input(t, "In 3-4").unwrap();
    assert_eq!(undo_len(&mut app), before + 1);
}

#[test]
fn plus_adds_after_the_last_device() {
    let mut app = app();
    let t = new_track(&mut app, json!({"kind": "audio"}));
    app.add_device(t, "gain").unwrap();
    app.add_device(t, "eq_7band").unwrap();
    app.remove_device(t, 0).unwrap();
    let r = app.add_device(t, "gain").unwrap();
    assert_eq!(r["slot"], 2, "after the last occupied slot, not into the gap");
    let order: Vec<&str> = app.engine.session().track(t).unwrap().mixer.inserts.iter().flatten().map(|i| i.plugin.as_str()).collect();
    assert_eq!(order, vec!["eq_7band", "gain"]);
}

#[test]
fn routing_menus_never_offer_a_loop() {
    use crate::mixer::{input_choices, output_choices, send_targets};
    let mut app = app();
    let a = new_track(&mut app, json!({"kind": "audio", "name": "A"}));
    app.focus(a);
    let fx1 = app.add_fx_channel().unwrap();
    let fx2 = app.new_fx_channel(None, 0.0).unwrap();
    let fx3 = app.new_fx_channel(None, 0.0).unwrap();
    // A -> FX 1 -> FX 2 -> FX 3.
    app.set_output(fx1, "FX 2").unwrap();
    app.set_output(fx2, "FX 3").unwrap();
    let s = app.engine.session_arc();
    let labels = |v: Vec<(String, String)>| v.into_iter().map(|(l, _)| l).collect::<Vec<_>>();
    let out3 = labels(output_choices(&s, s.track(fx3).unwrap()));
    assert!(!out3.iter().any(|l| l == "FX 1" || l == "FX 2" || l == "FX 3"), "three hops back: {out3:?}");
    let out2 = labels(output_choices(&s, s.track(fx2).unwrap()));
    assert!(!out2.contains(&"FX 1".to_string()) && out2.contains(&"FX 3".to_string()), "{out2:?}");
    let sends3 = labels(send_targets(&s, s.track(fx3).unwrap(), None));
    assert!(sends3.is_empty(), "an FX 3 send could only loop: {sends3:?}");
    let sends1 = labels(send_targets(&s, s.track(fx1).unwrap(), None));
    assert_eq!(sends1, vec!["FX 2", "FX 3"], "a bus may feed another bus downstream");
    let ins = labels(input_choices(&s, s.track(a).unwrap(), 2));
    assert!(ins.iter().all(|l| !l.starts_with("Bus")), "A feeds all three busses: {ins:?}");
    // The dropdown in the window agrees.
    app.ui.show_mixer = true;
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    let at = mid(strip(&app, fx3).output);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    let menu: Vec<String> = app.mixer.layout.menu.iter().map(|m| m.label.clone()).collect();
    assert!(menu.contains(&"Main".to_string()) && !menu.contains(&"FX 1".to_string()), "{menu:?}");
}

#[test]
fn the_device_chain_lists_the_sends_with_pre_and_post() {
    let (mut app, ctx, mut time, t) = mixer_app();
    app.focus(t);
    app.new_fx_channel(Some(t), -6.0).unwrap();
    app.focus(t);
    frames(&mut app, &ctx, 3, &mut time);
    let row = app.mixer.devices.sends.first().cloned().expect("a sends section");
    assert_eq!((row.target.as_str(), row.level_db, row.pre_fader), ("FX 1", -6.0, false));
    assert_ne!(app.mixer.devices.add_send, [0.0; 4]);
    click(&mut app, mid(row.pre));
    frames(&mut app, &ctx, 6, &mut time);
    assert!(app.engine.session().track(t).unwrap().mixer.sends[0].as_ref().unwrap().pre_fader);
    let at = mid(app.mixer.devices.sends[0].remove);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    assert!(app.engine.session().track(t).unwrap().mixer.sends.iter().all(Option::is_none));
}

fn click_right(app: &mut StudioApp, at: (f32, f32)) {
    let (req, _rx) = crate::ControlRequest::new("ui.click", json!({"x": at.0, "y": at.1, "button": "right"}));
    let ctx = egui::Context::default();
    let _ = crate::control::handle_for_test(app, &ctx, &req);
}

#[test]
fn a_send_target_click_retargets_and_a_right_click_has_pre_post_and_remove() {
    let (mut app, ctx, mut time, t) = mixer_app();
    app.new_fx_channel(Some(t), 0.0).unwrap();
    app.new_fx_channel(None, 0.0).unwrap();
    app.focus(t);
    frames(&mut app, &ctx, 3, &mut time);
    let send = |app: &StudioApp| app.engine.session().track(t).unwrap().mixer.sends[0].clone().unwrap();
    for wide in [false, true] {
        let menu_of = |app: &StudioApp| if wide { app.mixer.devices.menu.clone() } else { app.mixer.layout.menu.clone() };
        let row_of = |app: &StudioApp| if wide { app.mixer.devices.sends[0].clone() } else { strip(app, t).sends[0].clone() };
        let pre0 = send(&app).pre_fader;
        // A primary click opens the retarget menu and none of the context items.
        let at = mid(row_of(&app).target_rect);
        click(&mut app, at);
        frames(&mut app, &ctx, 6, &mut time);
        let menu = menu_of(&app);
        let labels: Vec<&str> = menu.iter().map(|m| m.label.as_str()).collect();
        assert!(labels.contains(&"FX 1") && labels.contains(&"FX 2"), "targets listed (wide={wide}): {labels:?}");
        assert!(!labels.iter().any(|l| l.ends_with("-fader") || *l == "Remove Send"), "no context items: {labels:?}");
        let cur = send(&app).target;
        let current = row_of(&app).target;
        let pick = menu.iter().find(|m| m.label != current && !m.value.is_empty()).cloned().expect("another target");
        click(&mut app, mid(pick.rect));
        frames(&mut app, &ctx, 6, &mut time);
        assert_ne!(send(&app).target, cur, "picking {} retargeted (wide={wide})", pick.label);
        // A right-click opens only the context items; Pre-fader toggles the send.
        let at = mid(row_of(&app).target_rect);
        click_right(&mut app, at);
        frames(&mut app, &ctx, 6, &mut time);
        let labels: Vec<String> = menu_of(&app).iter().map(|m| m.label.clone()).collect();
        assert_eq!(labels, ["Pre-fader", "Remove Send"], "only the context items (wide={wide})");
        let item = menu_of(&app).into_iter().find(|m| m.label == "Pre-fader").unwrap();
        click(&mut app, mid(item.rect));
        frames(&mut app, &ctx, 6, &mut time);
        assert_eq!(send(&app).pre_fader, !pre0, "Pre-fader toggled (wide={wide})");
        // Back to post for the next pass.
        let at = mid(row_of(&app).target_rect);
        click_right(&mut app, at);
        frames(&mut app, &ctx, 6, &mut time);
        let item = menu_of(&app).into_iter().find(|m| m.label == "Post-fader").expect("Post-fader");
        click(&mut app, mid(item.rect));
        frames(&mut app, &ctx, 6, &mut time);
        assert_eq!(send(&app).pre_fader, pre0);
    }
}

// ---- Mixer follow-up ----

#[test]
fn hosted_plugins_read_as_names_never_as_class_ids() {
    use crate::devices::hosted_label;
    let id = "vst3:75747241564153494A5550382D303030";
    assert_eq!(hosted_label(id, true, Some("Jup-8000 V"), false), "Jup-8000 V", "the cache's name wins");
    assert_eq!(hosted_label(id, true, None, true), "Scanning…", "while the scan runs");
    assert_eq!(hosted_label(id, true, None, false), "VST3 instrument", "unknown after the scan");
    assert_eq!(hosted_label("clap:com.x.y", true, None, false), "CLAP instrument");
    assert_eq!(hosted_label("clap:com.x.y", false, None, false), "CLAP effect");
    // Built-ins keep their names, and an unknown id that is not hosted keeps its tail.
    let mut app = app();
    assert_eq!(app.plugin_name(id, true), "VST3 instrument", "no scan in tests, so no cache entry");
    let builtin = app.builtin_plugins().into_iter().find(|p| p.instrument).expect("a built-in instrument");
    assert_eq!(app.plugin_name(&builtin.id, true), builtin.name);
}

fn send_control(app: &mut StudioApp, id: &str, p: Value) {
    let (req, _rx) = crate::ControlRequest::new(id, p);
    let ctx = egui::Context::default();
    let _ = crate::control::handle_for_test(app, &ctx, &req);
}

fn click_with(app: &mut StudioApp, at: (f32, f32), extra: Value) {
    let mut p = json!({"x": at.0, "y": at.1});
    if let (Some(o), Some(m)) = (p.as_object_mut(), extra.as_object()) {
        o.extend(m.clone());
    }
    send_control(app, "ui.click", p);
}

fn move_to(app: &mut StudioApp, at: (f32, f32)) {
    send_control(app, "ui.move", json!({"x": at.0, "y": at.1}));
}

/// One wheel notch (`dy` lines; positive scrolls up) over `at`.
fn wheel(app: &mut StudioApp, at: (f32, f32), dy: f32, extra: Value) {
    let mut p = json!({"x": at.0, "y": at.1, "dy": dy, "unit": "line", "steps": 1});
    if let (Some(o), Some(m)) = (p.as_object_mut(), extra.as_object()) {
        o.extend(m.clone());
    }
    send_control(app, "ui.scroll", p);
}

fn key(app: &mut StudioApp, name: &str) {
    send_control(app, "ui.key", json!({"key": name}));
}

fn typed(app: &mut StudioApp, text: &str) {
    send_control(app, "ui.text", json!({"text": text}));
}

fn volume(app: &StudioApp, t: TrackId) -> f32 {
    app.engine.session().track(t).unwrap().mixer.volume_db
}

fn send_db(app: &StudioApp, t: TrackId) -> f32 {
    app.engine.session().track(t).unwrap().mixer.sends[0].as_ref().unwrap().level_db
}

fn balance(app: &StudioApp, t: TrackId) -> f32 {
    crate::mixer::balance_of(&app.engine.session().track(t).unwrap().mixer.pan)
}

/// A mixer app whose track has one send, at 0 dB.
fn send_app() -> (StudioApp, egui::Context, f64, TrackId) {
    let (mut app, ctx, mut time, t) = mixer_app();
    app.new_fx_channel(Some(t), 0.0).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    (app, ctx, time, t)
}

#[test]
fn double_click_alt_click_ctrl_click_and_delete_reset_a_fader_one_undo_step_each() {
    let (mut app, ctx, mut time, t) = mixer_app();
    let at = mid(strip(&app, t).fader);
    for name in ["double-click", "alt-click", "ctrl-click", "delete"] {
        app.run("mix.volume", json!({"tracks": [t.0], "db": 0.2})).unwrap();
        frames(&mut app, &ctx, 3, &mut time);
        let before = undo_len(&mut app);
        match name {
            "double-click" => click_with(&mut app, at, json!({"count": 2})),
            "alt-click" => click_with(&mut app, at, json!({"alt": true})),
            "ctrl-click" => click_with(&mut app, at, json!({"ctrl": true})),
            _ => {
                move_to(&mut app, at);
                frames(&mut app, &ctx, 3, &mut time);
                key(&mut app, "Delete");
            }
        }
        frames(&mut app, &ctx, 12, &mut time);
        assert_eq!(volume(&app, t), 0.0, "{name} resets the fader to 0.0 dB");
        assert_eq!(undo_len(&mut app), before + 1, "{name}: one undo step");
        app.run("edit.undo", json!({})).unwrap();
        assert_eq!(volume(&app, t), 0.2, "{name}: undo brings the nudged value back");
    }
}

#[test]
fn double_click_resets_a_pan_knob_and_a_send_level() {
    let (mut app, ctx, mut time, t) = send_app();
    let knob = mid(strip(&app, t).knob);
    for (name, extra) in [("double-click", json!({"count": 2})), ("alt-click", json!({"alt": true}))] {
        app.set_balance(t, 0.3).unwrap();
        app.engine.end_merge();
        frames(&mut app, &ctx, 3, &mut time);
        let before = undo_len(&mut app);
        frames(&mut app, &ctx, 60, &mut time);
        click_with(&mut app, knob, extra);
        frames(&mut app, &ctx, 12, &mut time);
        assert_eq!(balance(&app, t), 0.0, "knob {name}");
        assert_eq!(app.engine.session().track(t).unwrap().mixer.pan, vec![-1.0, 1.0], "both panners back at center");
        assert_eq!(undo_len(&mut app), before + 1, "knob {name}: one undo step");
    }
    for (name, bar, extra) in [
        ("bar double-click", true, json!({"count": 2})),
        ("number alt-click", false, json!({"alt": true})),
        ("number ctrl-click", false, json!({"ctrl": true})),
    ] {
        app.run("mix.send_level", json!({"track": t.0, "slot": 0, "db": -3.5})).unwrap();
        frames(&mut app, &ctx, 3, &mut time);
        let row = strip(&app, t).sends[0].clone();
        let at = mid(if bar { row.level_rect } else { row.readout_rect });
        let before = undo_len(&mut app);
        frames(&mut app, &ctx, 60, &mut time);
        click_with(&mut app, at, extra);
        frames(&mut app, &ctx, 12, &mut time);
        assert_eq!(send_db(&app, t), 0.0, "send {name}");
        assert_eq!(undo_len(&mut app), before + 1, "send {name}: one undo step");
    }
    app.run("mix.send_level", json!({"track": t.0, "slot": 0, "db": -3.5})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let at = mid(strip(&app, t).sends[0].level_rect);
    move_to(&mut app, at);
    frames(&mut app, &ctx, 3, &mut time);
    key(&mut app, "Delete");
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(send_db(&app, t), 0.0, "Delete over the send level resets it");
}

#[test]
fn shift_drag_moves_a_tenth_as_far() {
    use crate::mixer::db_to_k;
    let (mut app, ctx, mut time, t) = send_app();
    let f = strip(&app, t).fader;
    let x = (f[0] + f[2]) / 2.0;
    let cap_y = f[3] - db_to_k(0.0) * (f[3] - f[1]);
    let mut moved = Vec::new();
    for mods in [json!({}), json!({"shift": true})] {
        app.run("mix.volume", json!({"tracks": [t.0], "db": 0.0})).unwrap();
        frames(&mut app, &ctx, 3, &mut time);
        drag(&mut app, (x, cap_y), (x, cap_y + 40.0), mods);
        frames(&mut app, &ctx, 30, &mut time);
        moved.push(db_to_k(0.0) - db_to_k(volume(&app, t)));
    }
    assert!(moved[0] > 0.1, "a plain drag moves the fader: {moved:?}");
    assert!((moved[1] / moved[0] - 0.1).abs() < 0.01, "shift-drag moves a tenth as far: {moved:?}");
    let knob = mid(strip(&app, t).knob);
    let mut moved = Vec::new();
    for mods in [json!({}), json!({"shift": true})] {
        app.set_balance(t, 0.0).unwrap();
        app.engine.end_merge();
        frames(&mut app, &ctx, 3, &mut time);
        drag(&mut app, knob, (knob.0, knob.1 - 40.0), mods);
        frames(&mut app, &ctx, 30, &mut time);
        moved.push(balance(&app, t));
    }
    assert!((moved[0] - 0.4).abs() < 0.02 && (moved[1] - 0.04).abs() < 0.005, "{moved:?}");
    let bar = mid(strip(&app, t).sends[0].level_rect);
    let mut moved = Vec::new();
    for mods in [json!({}), json!({"shift": true})] {
        app.run("mix.send_level", json!({"track": t.0, "slot": 0, "db": 0.0})).unwrap();
        frames(&mut app, &ctx, 3, &mut time);
        drag(&mut app, bar, (bar.0, bar.1 + 30.0), mods);
        frames(&mut app, &ctx, 30, &mut time);
        moved.push(db_to_k(0.0) - db_to_k(send_db(&app, t)));
    }
    assert!((moved[1] / moved[0] - 0.1).abs() < 0.01, "{moved:?}");
}

#[test]
fn a_slow_drag_snaps_to_a_mark_and_a_fast_one_does_not() {
    use crate::mixer::db_to_k;
    let (mut app, ctx, mut time, t) = send_app();
    let f = strip(&app, t).fader;
    let (x, h) = ((f[0] + f[2]) / 2.0, f[3] - f[1]);
    let cap_y = f[3] - db_to_k(0.0) * h;
    let travel = (db_to_k(0.0) - db_to_k(-6.1)) * h;
    for (steps, snaps) in [(150, true), (4, false)] {
        app.run("mix.volume", json!({"tracks": [t.0], "db": 0.0})).unwrap();
        frames(&mut app, &ctx, 3, &mut time);
        drag(&mut app, (x, cap_y), (x, cap_y + travel), json!({"steps": steps}));
        frames(&mut app, &ctx, 200, &mut time);
        let db = volume(&app, t);
        if snaps {
            assert_eq!(db, -6.0, "slow: snapped to -6 dB");
        } else {
            assert!((db + 6.1).abs() < 0.03, "fast: not snapped, {db}");
        }
    }
    let knob = mid(strip(&app, t).knob);
    for (steps, snaps) in [(150, true), (4, false)] {
        app.set_balance(t, 0.1).unwrap();
        app.engine.end_merge();
        frames(&mut app, &ctx, 3, &mut time);
        drag(&mut app, knob, (knob.0, knob.1 + 9.0), json!({"steps": steps}));
        frames(&mut app, &ctx, 200, &mut time);
        let b = balance(&app, t);
        if snaps {
            assert_eq!(b, 0.0, "slow: snapped to center");
        } else {
            assert!((b - 0.01).abs() < 0.003, "fast: {b}");
        }
    }
    let bar = mid(strip(&app, t).sends[0].level_rect);
    let up = (db_to_k(-0.1) - db_to_k(-4.0)) * 150.0;
    for (steps, snaps) in [(150, true), (4, false)] {
        app.run("mix.send_level", json!({"track": t.0, "slot": 0, "db": -4.0})).unwrap();
        frames(&mut app, &ctx, 3, &mut time);
        drag(&mut app, bar, (bar.0, bar.1 - up), json!({"steps": steps}));
        frames(&mut app, &ctx, 200, &mut time);
        let db = send_db(&app, t);
        if snaps {
            assert_eq!(db, 0.0, "slow: snapped to the 0 dB mark");
        } else {
            assert!((db + 0.1).abs() < 0.05, "fast: {db}");
        }
    }
}

#[test]
fn the_wheel_nudges_and_one_gesture_is_one_undo_step() {
    let (mut app, ctx, mut time, t) = send_app();
    let at = mid(strip(&app, t).fader);
    let before = undo_len(&mut app);
    for _ in 0..3 {
        wheel(&mut app, at, 1.0, json!({}));
        frames(&mut app, &ctx, 4, &mut time);
    }
    assert!((volume(&app, t) - 0.3).abs() < 1e-4, "three notches up: {}", volume(&app, t));
    assert_eq!(undo_len(&mut app), before + 1, "the notches merge into one undo step");
    wheel(&mut app, at, -1.0, json!({"shift": true}));
    frames(&mut app, &ctx, 4, &mut time);
    assert!((volume(&app, t) - 0.29).abs() < 1e-4, "Shift+wheel is 0.01 dB: {}", volume(&app, t));
    assert_eq!(undo_len(&mut app), before + 1);
    frames(&mut app, &ctx, 45, &mut time);
    wheel(&mut app, at, 1.0, json!({}));
    frames(&mut app, &ctx, 4, &mut time);
    assert_eq!(undo_len(&mut app), before + 2, "after the pause a new step");
    move_to(&mut app, (3.0, 3.0));
    frames(&mut app, &ctx, 4, &mut time);
    wheel(&mut app, at, 1.0, json!({}));
    frames(&mut app, &ctx, 4, &mut time);
    assert_eq!(undo_len(&mut app), before + 3, "after leaving the control a new step");
    assert!((volume(&app, t) - 0.49).abs() < 1e-3, "{}", volume(&app, t));
    app.run("edit.undo", json!({})).unwrap();
    assert!((volume(&app, t) - 0.39).abs() < 1e-3, "{}", volume(&app, t));
    let knob = mid(strip(&app, t).knob);
    wheel(&mut app, knob, 1.0, json!({}));
    frames(&mut app, &ctx, 4, &mut time);
    assert!((balance(&app, t) - 0.01).abs() < 1e-4, "{}", balance(&app, t));
    wheel(&mut app, knob, -1.0, json!({"shift": true}));
    frames(&mut app, &ctx, 4, &mut time);
    assert!((balance(&app, t) - 0.009).abs() < 1e-4, "Shift+wheel is a tenth of that: {}", balance(&app, t));
    let bar = mid(strip(&app, t).sends[0].level_rect);
    wheel(&mut app, bar, -1.0, json!({}));
    frames(&mut app, &ctx, 4, &mut time);
    assert!((send_db(&app, t) + 0.1).abs() < 1e-4, "{}", send_db(&app, t));
}

#[test]
fn the_wheel_over_a_control_does_not_scroll_the_strips() {
    let (mut app, ctx, mut time, t) = mixer_app();
    for _ in 0..30 {
        app.run("track.new", json!({"kind": "audio", "format": "stereo"})).unwrap();
    }
    frames(&mut app, &ctx, 3, &mut time);
    let at = mid(strip(&app, t).fader);
    wheel(&mut app, at, -150.0, json!({"unit": "point"}));
    frames(&mut app, &ctx, 8, &mut time);
    assert_eq!(app.mixer.hscroll, 0.0, "over a fader the wheel nudges it");
    let name = mid(strip(&app, t).name_rect);
    wheel(&mut app, name, -150.0, json!({"unit": "point"}));
    frames(&mut app, &ctx, 8, &mut time);
    assert!(app.mixer.hscroll > 0.0, "elsewhere on the strip it scrolls");
}

#[test]
fn parsing_what_a_readout_takes() {
    use crate::fine::{parse_db, parse_pan};
    for (text, want) in [
        ("-6", -6.0),
        ("-6.0", -6.0),
        ("-6 dB", -6.0),
        ("  +1.5 ", 1.5),
        ("+1.5dB", 1.5),
        ("0", 0.0),
        ("99", 12.0),
        ("-500", -144.0),
        ("-inf", -144.0),
    ] {
        assert_eq!(parse_db(text), Some(want), "{text:?}");
    }
    for bad in ["", "dB", "abc", "1,5", "--3", "nan"] {
        assert_eq!(parse_db(bad), None, "{bad:?}");
    }
    for (text, want) in
        [("L20", -0.2), ("R35", 0.35), ("C", 0.0), ("c", 0.0), ("-20", -0.2), ("20", 0.2), ("+20", 0.2), ("l 100", -1.0), ("R250", 1.0), ("50%", 0.5)]
    {
        assert!((parse_pan(text).unwrap() - want).abs() < 1e-6, "{text:?}");
    }
    for bad in ["", "L", "xyz", "LR5"] {
        assert_eq!(parse_pan(bad), None, "{bad:?}");
    }
    assert_eq!(crate::fine::pan_text(-0.2), "L20");
    assert_eq!(crate::fine::pan_text(0.0), "C");
}

/// Open a readout's field with a double-click, type `text`, and finish with `end` (a key name).
fn edit_readout(app: &mut StudioApp, ctx: &egui::Context, time: &mut f64, at: (f32, f32), text: &str, end: &str) {
    // Let the last click go stale, or this one counts as a triple click.
    frames(app, ctx, 60, time);
    click_with(app, at, json!({"count": 2}));
    frames(app, ctx, 12, time);
    assert!(app.mixer.edit.is_some(), "double-click opens the field");
    typed(app, text);
    frames(app, ctx, 3, time);
    key(app, end);
    frames(app, ctx, 6, time);
}

fn rects_overlap(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

#[test]
fn the_fader_readout_edits_in_place_and_applies_cancels_and_clamps() {
    let (mut app, ctx, mut time, t) = mixer_app();
    let ro = mid(strip(&app, t).readout_rect);
    let f = mid(strip(&app, t).fader);
    assert!(!rects_overlap(strip(&app, t).readout_rect, strip(&app, t).fader), "distinct hit rects");
    assert_eq!(strip(&app, t).volume_text, "+0.0");
    for (text, want) in [("-6", -6.0), ("-6.0", -6.0), ("-6 dB", -6.0), ("+1.5", 1.5), ("99", 12.0), ("-500", -144.0)] {
        let before = undo_len(&mut app);
        edit_readout(&mut app, &ctx, &mut time, ro, text, "Enter");
        assert_eq!(volume(&app, t), want, "{text:?} applies");
        if want != -6.0 || text == "-6" {
            assert_eq!(undo_len(&mut app), before + 1, "{text:?}: one undo step");
        }
        assert!(app.mixer.edit.is_none(), "Enter closes the field");
    }
    assert_eq!(strip(&app, t).volume_text, "-inf");
    app.run("mix.volume", json!({"tracks": [t.0], "db": 2.0})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let before = undo_len(&mut app);
    edit_readout(&mut app, &ctx, &mut time, ro, "-20", "Escape");
    assert_eq!((volume(&app, t), app.mixer.edit.is_none()), (2.0, true), "Escape cancels");
    frames(&mut app, &ctx, 60, &mut time);
    click_with(&mut app, ro, json!({"count": 2}));
    frames(&mut app, &ctx, 12, &mut time);
    typed(&mut app, "-20");
    frames(&mut app, &ctx, 3, &mut time);
    let name = mid(strip(&app, t).name_rect);
    click_with(&mut app, name, json!({}));
    frames(&mut app, &ctx, 8, &mut time);
    assert_eq!((volume(&app, t), app.mixer.edit.is_none()), (2.0, true), "losing focus cancels");
    edit_readout(&mut app, &ctx, &mut time, ro, "loud", "Enter");
    assert_eq!((volume(&app, t), app.mixer.edit.is_none()), (2.0, true), "garbage changes nothing");
    let status = app.status.clone();
    assert!(status.contains("not a level"), "{status}");
    assert_eq!(undo_len(&mut app), before, "no cancel made an undo step");
    frames(&mut app, &ctx, 60, &mut time);
    click_with(&mut app, ro, json!({"count": 2}));
    frames(&mut app, &ctx, 12, &mut time);
    let v = app.inspect(&ctx);
    assert_eq!(v["readout_edit"]["text"], json!("2.0"));
    assert!(v["readout_edit"]["rect"][2].as_f64().unwrap() > 0.0);
    key(&mut app, "Escape");
    frames(&mut app, &ctx, 6, &mut time);
    frames(&mut app, &ctx, 60, &mut time);
    click_with(&mut app, f, json!({"count": 2}));
    frames(&mut app, &ctx, 12, &mut time);
    assert_eq!(volume(&app, t), 0.0, "double-click on the fader is still the reset");
    assert!(app.mixer.edit.is_none());
}

#[test]
fn the_pan_and_send_readouts_edit_in_place() {
    let (mut app, ctx, mut time, t) = send_app();
    let pan = mid(strip(&app, t).pan_readout_rect);
    assert_eq!(strip(&app, t).pan_text, "C");
    for (text, want) in [("L20", -0.2), ("R35", 0.35), ("C", 0.0), ("-20", -0.2), ("20", 0.2), ("R250", 1.0)] {
        let before = undo_len(&mut app);
        edit_readout(&mut app, &ctx, &mut time, pan, text, "Enter");
        assert!((balance(&app, t) - want).abs() < 1e-5, "{text:?}: {}", balance(&app, t));
        let n = undo_len(&mut app);
        assert_eq!(n, before + 1, "{text:?}: both panners, one undo step");
    }
    edit_readout(&mut app, &ctx, &mut time, pan, "L50", "Escape");
    assert_eq!(balance(&app, t), 1.0, "Escape leaves the pan alone");
    let row = strip(&app, t).sends[0].clone();
    assert_eq!(row.level_text, "0");
    let num = mid(row.readout_rect);
    for (text, want) in [("-6", -6.0), ("+1.5", 1.5), ("40", 12.0), ("-7.25 dB", -7.25)] {
        edit_readout(&mut app, &ctx, &mut time, num, text, "Enter");
        assert_eq!(send_db(&app, t), want, "{text:?}");
    }
    edit_readout(&mut app, &ctx, &mut time, num, "3", "Escape");
    assert_eq!(send_db(&app, t), -7.25, "Escape leaves the send alone");
    app.focus(t);
    frames(&mut app, &ctx, 3, &mut time);
    let wide = mid(app.mixer.devices.sends[0].readout_rect);
    assert!(app.mixer.devices.sends[0].level_text.ends_with("dB"));
    edit_readout(&mut app, &ctx, &mut time, wide, "-1", "Enter");
    assert_eq!(send_db(&app, t), -1.0);
}

#[test]
fn inspect_exposes_the_readouts() {
    let (mut app, ctx, mut time, t) = send_app();
    app.run("mix.volume", json!({"tracks": [t.0], "db": -6.0})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let v = app.inspect(&ctx);
    let s = v["mixer"]["strips"].as_array().unwrap().iter().find(|s| s["track"] == json!(t.0)).unwrap().clone();
    assert_eq!(s["volume_text"], json!("-6.0"));
    assert!(s["readout_rect"][2].as_f64().unwrap() > s["readout_rect"][0].as_f64().unwrap());
    assert_eq!(s["pan_text"], json!("C"));
    assert!(s["pan_readout_rect"][3].as_f64().unwrap() > 0.0);
    assert_eq!(s["sends"][0]["level_text"], json!("0"));
    assert!(s["sends"][0]["readout_rect"][2].as_f64().unwrap() > 0.0);
}

#[test]
fn numbers_use_the_mono_medium_family_only_where_it_exists() {
    let ctx = egui::Context::default();
    run_probe(&ctx);
    assert_eq!(crate::theme::mono(&ctx, 12.0).family, egui::FontFamily::Monospace, "default fonts: Monospace");
    let mut defs = egui::FontDefinitions::default();
    let base = defs.families[&egui::FontFamily::Monospace].clone();
    defs.families.insert(egui::FontFamily::Name("mono-medium".into()), base);
    ctx.set_fonts(defs);
    for _ in 0..2 {
        run_probe(&ctx);
    }
    assert_eq!(crate::theme::mono(&ctx, 12.0).family, egui::FontFamily::Name("mono-medium".into()));
}

fn run_probe(ctx: &egui::Context) {
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::theme::probe_fonts(ui.ctx()));
    out.textures_delta.clear();
}

#[test]
fn the_master_or_main_out_strip_is_inside_the_mixer_pane_at_any_scale() {
    for ppp in [1.0f32, 1.5] {
        for master in [false, true] {
            let (mut app, ctx, mut time, _t) = mixer_app();
            ctx.set_pixels_per_point(ppp);
            app.new_fx_channel(None, 0.0).unwrap();
            if master {
                app.run("track.new", json!({"kind": "master"})).unwrap();
            }
            frames(&mut app, &ctx, 6, &mut time);
            let l = &app.mixer.layout;
            let m = l.master.clone().unwrap_or_else(|| panic!("no master strip (ppp {ppp}, master {master})"));
            let (a, r) = (l.area, m.rect);
            eprintln!("ppp {ppp} master {master}: area {a:?} strip {r:?} strips {}", l.strips.len());
            assert!(r[0] >= a[0] && r[2] <= a[2] && r[1] >= a[1] && r[3] <= a[3], "ppp {ppp}, master {master}: {r:?} not inside {a:?}");
            assert!(r[2] > r[0] && r[3] > r[1]);
            if master {
                assert_eq!(m.kind, Some(soundcraft_model::TrackKind::Master));
            } else {
                assert!(l.add_master[2] > l.add_master[0], "the offer to add a master fader");
            }
        }
    }
}

#[test]
fn a_strip_plus_adds_an_insert_to_that_track_whatever_is_focused() {
    let (mut app, ctx, mut time, a) = mixer_app();
    let r = app.run("track.new", json!({"kind": "audio", "format": "stereo", "name": "Vox"})).unwrap();
    let b = TrackId(r["tracks"][0].as_u64().unwrap());
    app.focus(a);
    frames(&mut app, &ctx, 3, &mut time);
    assert!(strip(&app, a).focused && !strip(&app, b).focused, "the focused strip is marked");
    let before = undo_len(&mut app);
    let plus = mid(strip(&app, b).add_insert);
    click(&mut app, plus);
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.mixer.layout.menu.iter().find(|m| m.value == "gain").cloned().expect("the strip + menu lists Gain");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    let s = app.engine.session();
    assert!(s.track(b).unwrap().mixer.inserts[0].is_some(), "Vox got the insert");
    assert!(s.track(a).unwrap().mixer.inserts.iter().all(Option::is_none), "the focused track did not");
    assert_eq!(undo_len(&mut app), before + 1);
    assert_eq!(strip(&app, b).inserts.len(), 1);
    assert_eq!(strip(&app, b).inserts[0].name, "Gain");
    // Right-click: bypass, then remove.
    let row = strip(&app, b).inserts[0].rect;
    click_right(&mut app, mid(row));
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.mixer.layout.menu.iter().find(|m| m.value == "bypass").cloned().expect("Bypass");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert!(app.engine.session().track(b).unwrap().mixer.inserts[0].as_ref().unwrap().bypass);
    let row = strip(&app, b).inserts[0].rect;
    click_right(&mut app, mid(row));
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.mixer.layout.menu.iter().find(|m| m.value == "remove").cloned().expect("Remove");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert!(app.engine.session().track(b).unwrap().mixer.inserts.iter().all(Option::is_none));
}

#[test]
fn insert_rows_cap_at_three_and_keep_every_fader_aligned() {
    let (mut app, ctx, mut time, a) = mixer_app();
    let r = app.run("track.new", json!({"kind": "audio", "format": "stereo", "name": "Vox"})).unwrap();
    let b = TrackId(r["tracks"][0].as_u64().unwrap());
    for _ in 0..5 {
        app.add_device(a, "gain").unwrap();
    }
    frames(&mut app, &ctx, 3, &mut time);
    let (sa, sb) = (strip(&app, a), strip(&app, b));
    assert_eq!(sa.inserts.len(), 3, "three rows and an n-more row");
    assert_eq!(sa.add_insert, [0.0; 4], "no + while the list overflows");
    assert_eq!((sa.fader[1], sa.fader[3]), (sb.fader[1], sb.fader[3]), "the faders line up with and without inserts");
    app.focus(a);
    frames(&mut app, &ctx, 3, &mut time);
    let more = [sa.rect[0] + 10.0, sa.inserts[2].rect[3] + 8.0];
    click(&mut app, (more[0], more[1]));
    frames(&mut app, &ctx, 4, &mut time);
    assert!(app.ui.show_devices, "n more… points at the device chain");
}

#[test]
fn clicking_anywhere_on_a_strip_focuses_it_and_the_highlight_follows() {
    let (mut app, ctx, mut time, a) = mixer_app();
    let r = app.run("track.new", json!({"kind": "audio", "format": "stereo", "name": "Vox"})).unwrap();
    let b = TrackId(r["tracks"][0].as_u64().unwrap());
    frames(&mut app, &ctx, 3, &mut time);
    for t in [b, a, b] {
        let s = strip(&app, t);
        // The strip's empty margin below the input row, not a control.
        click(&mut app, (s.rect[0] + 2.0, (s.rect[1] + s.rect[3]) / 2.0));
        frames(&mut app, &ctx, 4, &mut time);
        assert_eq!(app.focus_track(), Some(t));
        let other = if t == a { b } else { a };
        assert!(strip(&app, t).focused && !strip(&app, other).focused, "highlight follows focus");
    }
}

#[test]
fn the_peak_readout_holds_the_loudest_peak_until_clicked() {
    let (mut app, ctx, mut time, a) = mixer_app();
    let r = app.run("track.new", json!({"kind": "audio", "format": "stereo", "name": "Vox"})).unwrap();
    let b = TrackId(r["tracks"][0].as_u64().unwrap());
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(strip(&app, a).peak_text, "-inf");
    // A known peak (0.7 is -3.1 dBFS), then silence: the readout holds it.
    for (t, peak) in [(a, 0.7f32), (b, 1.2f32)] {
        let d = app.meters.entry(t).or_default();
        crate::host::feed_meter(d, [peak, 0.1], 0.0, 0.016);
        crate::host::feed_meter(d, [0.0, 0.0], 0.0, 0.016);
    }
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(strip(&app, a).peak_text, "-3.1");
    assert_eq!(strip(&app, b).peak_text, "+1.6", "over 0 dBFS reads with a plus sign");
    let d = app.meters.get_mut(&a).unwrap();
    crate::host::feed_meter(d, [0.5, 0.0], 0.0, 0.016);
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(strip(&app, a).peak_text, "-3.1", "a quieter peak does not lower the hold");
    // Click resets that strip only.
    let at = mid(strip(&app, a).peak_rect);
    click(&mut app, at);
    frames(&mut app, &ctx, 4, &mut time);
    assert_eq!(strip(&app, a).peak_text, "-inf");
    assert_eq!(strip(&app, b).peak_text, "+1.6");
    // Ctrl-click resets all.
    let at = mid(strip(&app, b).peak_rect);
    click_with(&mut app, at, json!({"ctrl": true}));
    frames(&mut app, &ctx, 4, &mut time);
    assert_eq!(strip(&app, b).peak_text, "-inf");
    assert_eq!(crate::mixer::peak_text(1.0), ("0.0".to_string(), true));
    assert_eq!(crate::mixer::peak_text(0.5), ("-6.0".to_string(), false));
}
