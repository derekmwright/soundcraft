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
