//! The minimal menus (File, Edit, Track, Transport, Audio), keyboard shortcuts, the status bar,
//! the save-changes question and the recovery offer.
//!
//! Shortcuts are our own map (plan 06, section 6), not the engine's Pro Tools strings: Ableton
//! where Ableton and Cubase/Logic differ.

use crate::theme::{STATUS_H, TOKENS};
use crate::{Dialog, StudioApp};
use egui::{Key, Modifiers};
use serde_json::{Value, json};

impl StudioApp {
    /// Run a session-replacing command or Quit, asking to save unsaved changes first.
    pub fn run_or_ask(&mut self, id: &str, params: Value) {
        if self.engine.transport.recording && matches!(id, "session.new" | "session.open" | "app.quit") {
            self.stop_play();
        }
        if self.engine.is_dirty() && matches!(id, "session.new" | "session.open" | "app.quit") {
            self.dialog = Some(Dialog::SaveChanges { then: (id.into(), params) });
            return;
        }
        let _ = self.run(id, params);
    }

    /// File › Save: to the session's file, or Save As when it has none. False when cancelled or failed.
    pub fn save(&mut self) -> bool {
        if self.engine.path.is_some() {
            return self.run("session.save", json!({})).is_ok();
        }
        self.save_as()
    }

    pub fn save_as(&mut self) -> bool {
        let name = format!("{}.scraft", self.engine.session().name);
        let Some(path) = self.services.pick_save.as_ref().and_then(|f| f(&name)) else { return false };
        self.run("session.save_as", json!({"path": path, "replace": true})).is_ok()
    }

    pub fn open_dialog(&mut self) {
        let Some(path) = self.services.pick_open.as_ref().and_then(|f| f("session.open")) else { return };
        self.run_or_ask("session.open", json!({"path": path}));
    }
}

pub fn menu_bar(app: &mut StudioApp, ui: &mut egui::Ui) {
    egui::Panel::top("menu_bar").exact_size(24.0).frame(egui::Frame::NONE.fill(TOKENS.panel).inner_margin(egui::Margin::symmetric(8, 2))).show(
        ui,
        |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New").clicked() {
                        app.run_or_ask("session.new", json!({}));
                        ui.close();
                    }
                    if ui.button("Open…").clicked() {
                        ui.close();
                        app.open_dialog();
                    }
                    if ui.button("Save").clicked() {
                        ui.close();
                        app.save();
                    }
                    if ui.button("Save As…").clicked() {
                        ui.close();
                        app.save_as();
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ui.close();
                        app.run_or_ask("app.quit", json!({}));
                    }
                });
                ui.menu_button("Edit", |ui| {
                    let items: [(&str, &str, fn(&mut StudioApp)); 8] = [
                        ("Undo", "Ctrl+Z", |a| drop(a.run("edit.undo", json!({})))),
                        ("Redo", "Ctrl+Shift+Z", |a| drop(a.run("edit.redo", json!({})))),
                        ("Cut", "Ctrl+X", |a| drop(a.copy_selected(true))),
                        ("Copy", "Ctrl+C", |a| drop(a.copy_selected(false))),
                        ("Paste at Cursor", "Ctrl+V", |a| drop(a.paste_at_cursor())),
                        ("Duplicate", "Ctrl+D", |a| drop(a.duplicate_selected())),
                        ("Split at Cursor", "Ctrl+E", |a| drop(a.split_at_cursor())),
                        ("Delete", "Del", |a| drop(a.delete_selected())),
                    ];
                    for (label, key, f) in items {
                        if ui.add(egui::Button::new(label).shortcut_text(key)).clicked() {
                            f(app);
                            ui.close();
                        }
                    }
                });
                ui.menu_button("Track", |ui| {
                    if ui.button("Add Audio Track").clicked() {
                        let _ = app.add_track("audio");
                        ui.close();
                    }
                    if ui.button("Add Instrument Track").clicked() {
                        let _ = app.add_track("instrument");
                        ui.close();
                    }
                    if ui.button("Add FX Channel").clicked() {
                        let _ = app.add_fx_channel();
                        ui.close();
                    }
                    if ui.button("New MIDI Clip at Cursor").clicked() {
                        let _ = app.new_midi_clip(None);
                        ui.close();
                    }
                });
                ui.menu_button("View", |ui| {
                    let u = &mut app.ui;
                    for (label, key, flag) in [
                        ("Mixer", "Ctrl+Alt+M", &mut u.show_mixer),
                        ("Devices", "Ctrl+Alt+L", &mut u.show_devices),
                        ("Full Mixer Window", "F3", &mut u.mixer_full),
                    ] {
                        let text = if *flag { format!("✓ {label}") } else { format!("   {label}") };
                        if ui.add(egui::Button::new(text).shortcut_text(key)).clicked() {
                            *flag = !*flag;
                            ui.close();
                        }
                    }
                });
                ui.menu_button("Transport", |ui| {
                    let cycle = app.engine.session().edit.loop_playback;
                    let items: [(&str, &str, &str, Value); 4] = [
                        ("Play / Stop", "Space", "transport.toggle", json!({})),
                        ("Record", "R", "transport.record", json!({})),
                        ("Return to Zero", "Home", "transport.rtz", json!({})),
                        (if cycle { "Cycle Off" } else { "Cycle On" }, "C", "options.loop_playback", json!({"value": !cycle})),
                    ];
                    for (label, key, id, p) in items {
                        if ui.add(egui::Button::new(label).shortcut_text(key)).clicked() {
                            let _ = app.run(id, p);
                            ui.close();
                        }
                    }
                });
                ui.menu_button("Audio", |ui| {
                    let name = app.player.as_ref().map_or("No audio device (--no-audio)".to_string(), |p| p.device_name.clone());
                    ui.label(egui::RichText::new(name).color(TOKENS.text_dim));
                    if ui.add_enabled(app.player.is_some(), egui::Button::new("Reconnect Device")).clicked() {
                        app.reconnect_audio();
                        ui.close();
                    }
                    if ui.button("Scan Plug-ins").clicked() {
                        crate::plugin_scan::rescan();
                        ui.close();
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let s = app.engine.session();
                    let dirty = if app.engine.is_dirty() { " •" } else { "" };
                    ui.label(egui::RichText::new(format!("{}{dirty}", s.name)).color(TOKENS.text_dim));
                });
            });
        },
    );
}

pub fn status_bar(app: &mut StudioApp, ui: &mut egui::Ui) {
    egui::Panel::bottom("status").exact_size(STATUS_H).frame(egui::Frame::NONE.fill(TOKENS.panel).inner_margin(egui::Margin::symmetric(10, 2))).show(
        ui,
        |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(egui::RichText::new(&app.status).color(TOKENS.text_dim).size(11.5));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let c = app.cursor();
                    let text = format!("Cursor {}  ·  {}", crate::transport::bars_beats(app, c).trim(), crate::transport::min_sec(app, c));
                    ui.label(egui::RichText::new(text).color(TOKENS.text_dim).size(11.5).monospace());
                });
            });
        },
    );
}

fn pressed(ctx: &egui::Context, mods: Modifiers, key: Key) -> bool {
    ctx.input_mut(|i| i.consume_key(mods, key))
}

/// Keyboard shortcuts, unless a text field has the keyboard.
pub fn shortcuts(app: &mut StudioApp, ctx: &egui::Context) {
    if ctx.egui_wants_keyboard_input() || app.dialog.is_some() {
        return;
    }
    let cmd = Modifiers::COMMAND;
    let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
    // egui turns the platform's copy/cut/paste keys into these events.
    let (copy, cut, paste) = ctx.input(|i| {
        let has = |f: fn(&egui::Event) -> bool| i.events.iter().any(f);
        (has(|e| matches!(e, egui::Event::Copy)), has(|e| matches!(e, egui::Event::Cut)), has(|e| matches!(e, egui::Event::Paste(_))))
    });
    if copy || pressed(ctx, cmd, Key::C) {
        let _ = app.copy_selected(false);
    }
    if cut || pressed(ctx, cmd, Key::X) {
        let _ = app.copy_selected(true);
    }
    if paste || pressed(ctx, cmd, Key::V) {
        let _ = app.paste_at_cursor();
    }
    if pressed(ctx, cmd_shift, Key::Z) || pressed(ctx, cmd, Key::Y) {
        let _ = app.run("edit.redo", json!({}));
    }
    if pressed(ctx, cmd, Key::Z) {
        let _ = app.run("edit.undo", json!({}));
    }
    if pressed(ctx, cmd, Key::D) {
        let _ = app.duplicate_selected();
    }
    if pressed(ctx, cmd, Key::E) || pressed(ctx, cmd, Key::T) {
        let _ = app.split_at_cursor();
    }
    if pressed(ctx, cmd_shift, Key::S) {
        app.save_as();
    }
    if pressed(ctx, cmd, Key::S) {
        app.save();
    }
    if pressed(ctx, cmd, Key::O) {
        app.open_dialog();
    }
    if pressed(ctx, cmd, Key::N) {
        app.run_or_ask("session.new", json!({}));
    }
    if pressed(ctx, cmd, Key::R) || pressed(ctx, Modifiers::NONE, Key::R) || pressed(ctx, Modifiers::NONE, Key::F9) {
        let _ = app.run("transport.record", json!({}));
    }
    if pressed(ctx, Modifiers::NONE, Key::Space) {
        let _ = app.run("transport.toggle", json!({}));
    }
    if pressed(ctx, Modifiers::NONE, Key::Home) || pressed(ctx, Modifiers::NONE, Key::Enter) {
        let _ = app.run("transport.rtz", json!({}));
    }
    if pressed(ctx, Modifiers::NONE, Key::C) {
        let on = app.engine.session().edit.loop_playback;
        let _ = app.run("options.loop_playback", json!({"value": !on}));
    }
    // Delete over a fader, knob or send level resets it (Ableton); anywhere else it deletes.
    if pressed(ctx, Modifiers::NONE, Key::Delete) {
        match app.mixer.hover {
            Some(t) => app.reset_control(t),
            None => drop(app.delete_selected()),
        }
    } else if pressed(ctx, Modifiers::NONE, Key::Backspace) {
        let _ = app.delete_selected();
    }
    // Views (Ableton where it has a key: Ctrl+Alt+M mixer, Ctrl+Alt+L detail view; Cubase's F3
    // MixConsole for the full mixer, which Ableton has no key for).
    let cmd_alt = Modifiers::COMMAND | Modifiers::ALT;
    if pressed(ctx, cmd_alt, Key::M) {
        app.ui.show_mixer = !app.ui.show_mixer;
    }
    if pressed(ctx, cmd_alt, Key::L) {
        app.ui.show_devices = !app.ui.show_devices;
    }
    if pressed(ctx, Modifiers::NONE, Key::F3) {
        app.ui.mixer_full = !app.ui.mixer_full;
    }
    if pressed(ctx, Modifiers::NONE, Key::Escape) {
        let _ = app.run("edit.select_none", json!({}));
    }
    if pressed(ctx, Modifiers::NONE, Key::Plus) || pressed(ctx, Modifiers::NONE, Key::Equals) {
        let _ = app.engine.execute("view.zoom_in", &json!({}));
    }
    if pressed(ctx, Modifiers::NONE, Key::Minus) {
        let _ = app.engine.execute("view.zoom_out", &json!({}));
    }
}

/// The save-changes question.
pub fn dialogs(app: &mut StudioApp, ctx: &egui::Context) {
    let Some(Dialog::SaveChanges { then }) = app.dialog.clone() else { return };
    let mut choice = None;
    egui::Modal::new(egui::Id::new("save_changes")).show(ctx, |ui| {
        ui.set_width(340.0);
        ui.label(egui::RichText::new(format!("Save changes to “{}”?", app.engine.session().name)).strong());
        ui.label(egui::RichText::new("Your changes are lost if you don't save them.").color(TOKENS.text_dim));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                choice = Some(1);
            }
            if ui.button("Don't Save").clicked() {
                choice = Some(2);
            }
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                choice = Some(0);
            }
        });
    });
    match choice {
        Some(1) => {
            app.dialog = None;
            if app.save() {
                let _ = app.run(&then.0, then.1);
            }
        }
        Some(2) => {
            app.dialog = None;
            let _ = app.run(&then.0, then.1);
        }
        Some(_) => app.dialog = None,
        None => {}
    }
}

/// Unsaved work an earlier run left behind, offered once at launch.
pub fn recovery(app: &mut StudioApp, ctx: &egui::Context) {
    if app.leftovers.is_empty() {
        return;
    }
    let mut act: Option<(usize, bool)> = None;
    let mut later = false;
    egui::Window::new("Recover unsaved work").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        ui.label("An earlier run left unsaved changes:");
        for (i, l) in app.leftovers.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(&l.name);
                if ui.button("Recover").clicked() {
                    act = Some((i, true));
                }
                if ui.button("Discard").clicked() {
                    act = Some((i, false));
                }
            });
        }
        if ui.button("Later").clicked() {
            later = true;
        }
    });
    if let Some((i, recover)) = act
        && i < app.leftovers.len()
    {
        let left = app.leftovers.remove(i);
        if recover {
            app.recover(&left);
        } else {
            soundcraft_engine::autosave::discard(&left);
        }
    }
    if later {
        app.leftovers.clear();
    }
}
