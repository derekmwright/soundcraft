//! Slice 5 tests (icons, theme, shortcut map). Driven like a user: synthetic clicks and keys
//! through the control channel's input queue, one frame at a time. No player, no files.

use crate::{Services, StudioApp};
use serde_json::{Value, json};

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
}

fn mid(r: [f32; 4]) -> (f32, f32) {
    ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
}

impl T {
    fn new() -> T {
        let mut app = StudioApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.run("track.new", json!({"kind": "instrument"})).unwrap();
        let mut t = T { app, ctx: egui::Context::default(), time: 0.0 };
        t.frames(3);
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

    fn click(&mut self, at: (f32, f32)) {
        self.input("ui.click", json!({"x": at.0, "y": at.1}));
    }

    fn inspect(&self) -> Value {
        self.app.inspect(&self.ctx)
    }
}

#[test]
fn transport_buttons_show_lucide_icons_and_still_work() {
    let mut t = T::new();
    let icons = t.inspect()["layout"]["transport_icons"].clone();
    assert_eq!(icons, json!({"rtz": "skip-back", "stop": "square", "play": "play", "record": "circle", "cycle": "repeat"}));
    let play = t.app.arrange.layout.transport["play"];
    t.click(mid(play));
    assert!(t.app.is_playing(), "the Play icon button plays");
    let stop = t.app.arrange.layout.transport["stop"];
    t.click(mid(stop));
    assert!(!t.app.is_playing());
    let cycle = t.app.arrange.layout.transport["cycle"];
    t.click(mid(cycle));
    assert!(t.app.engine.session().edit.loop_playback, "the Loop icon button turns cycle on");
}

#[test]
fn icons_are_rasterized_once_not_every_frame() {
    let mut t = T::new();
    let first = crate::icons::rasterized(&t.ctx);
    assert!(first >= 5, "the transport's five icons at least: {first}");
    t.frames(30);
    assert_eq!(crate::icons::rasterized(&t.ctx), first, "no per-frame parsing");
}

/// The View menu's entry `label` (open the View menu first).
fn view_item(t: &T, label: &str) -> [f32; 4] {
    t.app.arrange.bar_menu.iter().find(|m| m.label == label).unwrap_or_else(|| panic!("no {label}: {:?}", t.app.arrange.bar_menu)).rect
}

/// View > Appearance > `label`, by clicks.
fn pick_appearance(t: &mut T, label: &str) {
    t.click(mid(view_item(t, "View")));
    t.click(mid(view_item(t, "Appearance")));
    t.click(mid(view_item(t, label)));
}

#[test]
fn view_appearance_switches_the_theme_by_clicks_and_never_dirties() {
    let mut t = T::new();
    assert_eq!(t.app.ui.appearance, crate::theme::Appearance::System);
    assert_eq!(t.inspect()["theme"], "dark", "no OS theme in a test: egui falls back to dark");
    let dirty = t.app.engine.is_dirty();
    let rev = t.app.engine.revision;
    pick_appearance(&mut t, "Light");
    assert_eq!(t.app.ui.appearance, crate::theme::Appearance::Light);
    assert_eq!(t.inspect()["theme"], "light");
    assert_eq!(t.inspect()["ui"]["appearance"], "light", "kept in UiState");
    assert_eq!(t.ctx.theme(), egui::Theme::Light, "egui's own widgets follow");
    assert_eq!(t.ctx.global_style().visuals.panel_fill, crate::theme::LIGHT.panel);
    pick_appearance(&mut t, "Dark");
    assert_eq!(t.inspect()["theme"], "dark");
    assert_eq!(t.ctx.global_style().visuals.panel_fill, crate::theme::DARK.panel);
    assert_eq!((t.app.engine.is_dirty(), t.app.engine.revision), (dirty, rev), "appearance is view state, not the document");
}

#[test]
fn system_appearance_follows_the_os_theme() {
    let mut t = T::new();
    t.ctx.options_mut(|o| o.fallback_theme = egui::Theme::Light);
    t.frames(2);
    assert_eq!(t.inspect()["theme"], "light", "System follows what egui resolves for the OS");
    pick_appearance(&mut t, "Dark");
    assert_eq!(t.inspect()["theme"], "dark", "a pinned theme ignores the OS");
}

#[test]
fn appearance_round_trips_through_ui_state_json() {
    let ui = crate::UiState { appearance: crate::theme::Appearance::Light, ..Default::default() };
    let back: crate::UiState = serde_json::from_value(serde_json::to_value(&ui).unwrap()).unwrap();
    assert_eq!(back.appearance, crate::theme::Appearance::Light);
    let old: crate::UiState = serde_json::from_value(json!({"track_height": 50.0})).unwrap();
    assert_eq!(old.appearance, crate::theme::Appearance::System, "an older ui.json follows the OS");
}

#[test]
fn both_themes_keep_text_meters_and_accent_legible() {
    use crate::theme::{DARK, LIGHT, Tokens, contrast};
    for (name, t) in [("dark", &DARK), ("light", &LIGHT)] {
        let t: &Tokens = t;
        for (bg_name, bg) in
            [("bg", t.bg), ("panel", t.panel), ("raised", t.panel_raised), ("header", t.header), ("lane", t.lane), ("button", t.button)]
        {
            assert!(contrast(t.text, bg) >= 7.0, "{name}: text on {bg_name}");
            assert!(contrast(t.text_dim, bg) >= 4.5, "{name}: dim text on {bg_name}: {}", contrast(t.text_dim, bg));
        }
        assert!(contrast(t.text, t.header_selected) >= 7.0, "{name}: text on a selected header");
        for bg in [t.bg, t.panel, t.panel_raised, t.lane] {
            assert!(contrast(t.accent, bg) >= 3.0, "{name}: accent {}", contrast(t.accent, bg));
            assert!(contrast(t.focus_ring, bg) >= 3.0, "{name}: focus ring");
            assert!(contrast(t.cursor, bg) >= 3.0, "{name}: playhead");
        }
        // Meters: level and clip cap on the meter's background.
        assert!(contrast(t.monitor, t.bg) >= 3.0, "{name}: meter level");
        assert!(contrast(t.record, t.bg) >= 3.0, "{name}: clip cap");
        // State fills carry their letter at 4.5:1.
        crate::theme::set_current(if t.light_clips { egui::Theme::Light } else { egui::Theme::Dark });
        for c in [t.mute, t.solo, t.record, t.monitor, t.accent] {
            assert!(contrast(crate::theme::text_on(c), c) >= 4.5, "{name}: letter on {c:?}");
        }
    }
    crate::theme::set_current(egui::Theme::Dark);
}

#[test]
fn clip_names_read_on_every_track_color_in_both_themes() {
    use crate::theme::{clip_colors, contrast, rgb, set_current};
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        set_current(theme);
        for c in soundcraft_model::TRACK_COLORS {
            for selected in [false, true] {
                for base in [rgb(c), crate::theme::darken(rgb(c), 0.5)] {
                    let (_, strip, ink) = clip_colors(base, selected);
                    assert!(contrast(ink, strip) >= 4.5, "{theme:?} {c:?} selected={selected}: {}", contrast(ink, strip));
                }
            }
        }
    }
    set_current(egui::Theme::Dark);
}

#[test]
fn tokens_are_the_only_colors() {
    // Every literal color lives in theme.rs (transparent mesh fringes aside).
    for (file, src) in [
        ("arrange.rs", include_str!("arrange.rs")),
        ("browser.rs", include_str!("browser.rs")),
        ("devices.rs", include_str!("devices.rs")),
        ("groups.rs", include_str!("groups.rs")),
        ("keys.rs", include_str!("keys.rs")),
        ("menus.rs", include_str!("menus.rs")),
        ("mixer.rs", include_str!("mixer.rs")),
        ("transport.rs", include_str!("transport.rs")),
        ("waveform.rs", include_str!("waveform.rs")),
        ("icons.rs", include_str!("icons.rs")),
        ("shortcuts.rs", include_str!("shortcuts.rs")),
    ] {
        for pat in ["Color32::from_", "Color32::WHITE", "Color32::BLACK", "Color32::RED", "Color32::GRAY", "Color32::LIGHT", "Color32::DARK"] {
            assert!(!src.contains(pat), "{file} paints a literal color ({pat})");
        }
    }
}

#[test]
fn no_two_shortcuts_share_a_key_in_a_scope() {
    use crate::shortcuts::{Scope, TABLE, text};
    let mut seen = std::collections::HashMap::new();
    for s in TABLE {
        let k = (s.scope, text(s));
        assert!(seen.insert(k.clone(), s.label).is_none(), "{k:?}: {} and {}", seen[&k], s.label);
        assert!(!s.note.is_empty(), "{}: every row says where its key comes from", s.label);
    }
    // A global key must not also mean something in the arrangement or the piano roll.
    for g in TABLE.iter().filter(|s| s.scope == Scope::Global) {
        for o in TABLE.iter().filter(|s| matches!(s.scope, Scope::Arrangement | Scope::PianoRoll)) {
            assert_ne!(text(g), text(o), "{} (global) and {} ({:?})", g.label, o.label, o.scope);
        }
    }
}

#[test]
fn every_handler_takes_its_keys_from_the_table() {
    let sources = [
        include_str!("menus.rs"),
        include_str!("groups.rs"),
        include_str!("browser.rs"),
        include_str!("keys.rs"),
        include_str!("mixer.rs"),
        include_str!("arrange.rs"),
        include_str!("devices.rs"),
        include_str!("transport.rs"),
        include_str!("lib.rs"),
    ];
    for src in sources {
        for raw in ["consume_key(", "key_pressed(", "shortcut_text(\"", "Key::"] {
            assert!(!src.contains(raw), "a handler reads keys directly ({raw})");
        }
    }
    // And every action in the table has a handler.
    let all: String = sources.concat();
    for s in crate::shortcuts::TABLE {
        let name = format!("{:?}", s.action);
        assert!(all.contains(&format!("A::{name}")) || all.contains(&format!("Action::{name}")), "{name} has no handler");
    }
}

#[test]
fn menus_show_the_table_keys() {
    use crate::shortcuts::{Action, label};
    assert_eq!(label(Action::Redo), "Ctrl+Shift+Z");
    assert_eq!(label(Action::Split), "Ctrl+E");
    assert_eq!(label(Action::ToggleBrowser), "Ctrl+Alt+B");
    assert_eq!(label(Action::Record), "R");
    assert_eq!(label(Action::NotesTransposeUp), "Ctrl+Up");
}

#[test]
fn keys_from_the_table_still_drive_the_transport_and_views() {
    let mut t = T::new();
    t.input("ui.key", json!({"key": "C"}));
    assert!(t.app.engine.session().edit.loop_playback, "C: cycle");
    t.input("ui.key", json!({"key": "Space"}));
    assert!(t.app.is_playing(), "Space: play");
    t.input("ui.key", json!({"key": "Space"}));
    assert!(!t.app.is_playing());
    let shown = t.app.ui.browser.show;
    t.input("ui.key", json!({"key": "B", "cmd": true, "alt": true}));
    assert_eq!(t.app.ui.browser.show, !shown, "Ctrl+Alt+B: browser");
    t.input("ui.key", json!({"key": "F3"}));
    assert!(t.app.ui.mixer_full, "F3: full mixer");
}

#[test]
fn help_keyboard_shortcuts_opens_groups_by_scope_and_filters() {
    let mut t = T::new();
    let rev = (t.app.engine.is_dirty(), t.app.engine.revision);
    t.click(mid(view_item(&t, "Help")));
    t.click(mid(view_item(&t, "Keyboard Shortcuts")));
    let w = t.inspect()["shortcuts"].clone();
    assert_eq!(w["open"], true);
    let scopes: Vec<String> = serde_json::from_value(w["scopes"].clone()).unwrap();
    assert_eq!(scopes, ["Global", "Arrangement", "Piano roll", "Browser", "Mixer", "Dialogs"]);
    let all = w["rows"].as_array().unwrap().len();
    assert_eq!(all, crate::shortcuts::TABLE.len(), "every row shown");
    // Filter by typing in its box.
    let filter: [f32; 4] = serde_json::from_value(w["filter"].clone()).unwrap();
    let spp = t.app.arrange.layout.samples_per_px;
    t.click(mid(filter));
    t.input("ui.text", json!({"text": "zoom-="}));
    assert_eq!(t.app.arrange.layout.samples_per_px, spp, "typing - and = in the filter is not a shortcut");
    t.app.ui.shortcuts_filter = "zoom".into();
    t.frames(2);
    let rows: Vec<String> = serde_json::from_value(t.inspect()["shortcuts"]["rows"].clone()).unwrap();
    assert_eq!(rows, ["Arrangement · Zoom In · +", "Arrangement · Zoom In · =", "Arrangement · Zoom Out · -"]);
    t.app.ui.shortcuts_filter.clear();
    t.click(mid(filter));
    t.input("ui.text", json!({"text": "ctrl+e"}));
    let rows: Vec<String> = serde_json::from_value(t.inspect()["shortcuts"]["rows"].clone()).unwrap();
    assert_eq!(rows, ["Arrangement · Split at Cursor · Ctrl+E"]);
    assert_eq!((t.app.engine.is_dirty(), t.app.engine.revision), rev, "the window never touches the document");
}

#[test]
fn the_shortcuts_window_closes_from_its_title_bar() {
    let mut t = T::new();
    t.click(mid(view_item(&t, "Help")));
    t.click(mid(view_item(&t, "Keyboard Shortcuts")));
    let w: [f32; 4] = serde_json::from_value(t.inspect()["shortcuts"]["window"].clone()).unwrap();
    t.click((w[2] - 14.0, w[1] + 13.0));
    assert_eq!(t.inspect()["shortcuts"]["open"], false);
    assert!(!t.app.ui.show_shortcuts);
}
