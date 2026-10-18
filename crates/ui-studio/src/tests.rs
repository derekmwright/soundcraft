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
