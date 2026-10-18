//! transitDAW's studio UI: an arrangement window and a transport over the SoundCraft engine.
//!
//! "Cubase bones, Ableton manners" (ADR 0008). The UI draws engine state and acts by executing
//! engine commands; audio goes through [`soundcraft_playback::Player`] and the host loop in
//! [`host`], which is a marked copy of `soundcraft-ui-egui`'s `SoundApp` loop (plan 06, section 3).
//! Nothing here depends on `soundcraft-ui-egui`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod actions;
pub mod arrange;
pub mod control;
pub mod devices;
pub mod fine;
pub mod host;
pub mod menus;
pub mod mixer;
pub mod plugin_scan;
pub mod theme;
pub mod transport;
pub mod waveform;

use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_model::TrackId;
use soundcraft_playback::Player;
use soundcraft_time::{Range, Samples};
use std::collections::HashMap;
use std::sync::mpsc::Receiver;

pub use control::ControlRequest;

/// Our own UI state (serde, so agents can read and set it; saved as `ui.json` in the transit
/// settings folder). Not `soundcraft-ui-egui`'s `UiState` (plan 06, L13).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct UiState {
    /// The audio device choice, kept with the preferences: per machine.
    pub audio: soundcraft_playback::AudioConfig,
    /// The user's own VST3 and CLAP folders.
    pub plugin_dirs: plugin_scan::PluginDirs,
    /// Plug-in editors to have open, as (track, slot); the instrument is
    /// `soundcraft_mix::INSTRUMENT_SLOT`. `ui.set` opens and closes editors through this list
    /// (the same field `tools/exp-quit.ps1` sets in SoundCraft).
    pub plugin_windows: Vec<(TrackId, usize)>,
    /// Track row height in points (plan 06, L9: ours, not the model's seven sizes).
    pub track_height: f32,
    /// Width of the track list.
    pub header_width: f32,
    /// The main window's size (logical points) and maximized state.
    pub window_size: [f32; 2],
    pub window_maximized: bool,
    /// The mixer as a pane under the arrangement (Ableton's Ctrl+Alt+M).
    pub show_mixer: bool,
    /// The device chain of the focused track under the arrangement (Ableton's detail view, Ctrl+Alt+L).
    pub show_devices: bool,
    /// The mixer fills the window instead of the arrangement (Cubase's F3).
    pub mixer_full: bool,
    /// Height of the mixer pane in points.
    pub mixer_height: f32,
    /// The edit cursor: where Play starts and Paste lands. View state of the open session, not
    /// part of the document (setting it never dirties it) and not saved with the preferences.
    #[serde(skip)]
    pub cursor: Samples,
    /// The track the cursor was last set on (paste target).
    #[serde(skip)]
    pub cursor_track: Option<TrackId>,
    /// The focused track: the one the device chain shows and the mixer and track list highlight.
    /// Follows clicks in both views. View state, never in the document.
    #[serde(skip)]
    pub focus_track: Option<TrackId>,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            audio: soundcraft_playback::AudioConfig::default(),
            plugin_dirs: plugin_scan::PluginDirs::default(),
            plugin_windows: Vec::new(),
            track_height: 46.0,
            header_width: 236.0,
            window_size: [1600.0, 1000.0],
            window_maximized: false,
            show_mixer: false,
            show_devices: true,
            mixer_full: false,
            mixer_height: 420.0,
            cursor: 0,
            cursor_track: None,
            focus_track: None,
        }
    }
}

/// Host services (file dialogs) injected by the app; absent in tests.
#[derive(Default)]
pub struct Services {
    pub pick_open: Option<Box<dyn Fn(&str) -> Option<String>>>,
    pub pick_save: Option<Box<dyn Fn(&str) -> Option<String>>>,
}

/// Ballistic meter display state (copied from `soundcraft-ui-egui`, see [`host::feed_meter`]).
#[derive(Debug, Clone, Copy, Default)]
pub struct MeterDisplay {
    pub level: [f32; 2],
    pub hold: [f32; 2],
    pub hold_age: f32,
    pub clip: bool,
    pub gr: f32,
    pub plugin_failures: u32,
}

/// A modal question.
#[derive(Debug, Clone)]
pub enum Dialog {
    /// Save changes before running `then` (session.new/open, app.quit).
    SaveChanges { then: (String, Value) },
}

/// An editor asked for (by `ui.set` or a menu) and not open yet: retried until the plug-in is
/// loaded, or given up at `deadline` (seconds of UI time).
#[derive(Debug, Clone, Copy)]
struct EditorWanted {
    key: (TrackId, usize),
    deadline: f64,
    next_try: f64,
    opened: bool,
}

pub struct StudioApp {
    pub engine: Engine,
    pub player: Option<Player>,
    /// Input capture, opened on first record.
    pub recorder: Option<soundcraft_playback::record::Recorder>,
    record_start: Samples,
    /// Punch range to trim the take to (loop recording only in this UI).
    punch: Option<Range>,
    recorder_failed: bool,
    lost_shown: u64,
    take_plan: Option<soundcraft_engine::io::Recording>,
    pub ui: UiState,
    /// The status line.
    pub status: String,
    pub services: Services,
    pub autosave_dir: Option<std::path::PathBuf>,
    autosaver: Option<soundcraft_engine::autosave::Autosaver>,
    autosaved_rev: u64,
    pub leftovers: Vec<soundcraft_engine::autosave::Leftover>,
    panic_rev: u64,
    last_autosave: Option<std::time::Instant>,
    pub main_meter: MeterDisplay,
    pub meters: HashMap<TrackId, MeterDisplay>,
    meter_snapshot: soundcraft_playback::MeterSnapshot,
    last_meter_poll: f64,
    /// Transport simulation when there is no player (tests, `--no-audio`).
    sim: Option<(Samples, Option<Samples>, Option<Range>)>,
    record_hint: Option<(f64, &'static str)>,
    pub quit_requested: bool,
    pub frame_ms: f32,
    last_frame: Option<f64>,
    last_rev: u64,
    control_rx: Option<Receiver<ControlRequest>>,
    pending_shots: Vec<control::PendingShot>,
    pub synthetic: Vec<egui::Event>,
    synthetic_grace: u32,
    /// Modifiers held by the synthetic gesture being played (`ui.drag {alt: true}`).
    synthetic_mods: Option<egui::Modifiers>,
    theme_applied: bool,
    pub dialog: Option<Dialog>,
    editors: Vec<EditorWanted>,
    /// The arrangement: gesture in progress, scroll, and last frame's layout (for `ui.inspect`).
    pub arrange: arrange::ArrangeState,
    /// What the last Copy took, so a Paste onto the original is refused (UX gap, plan 06).
    pub copied: Vec<(TrackId, Range)>,
    /// The mixer and device chain: gesture in progress, scroll, and last frame's layout.
    pub mixer: mixer::MixerState,
}

impl StudioApp {
    pub fn new(engine: Engine, player: Option<Player>, services: Services) -> Self {
        StudioApp {
            engine,
            player,
            recorder: None,
            record_start: 0,
            punch: None,
            recorder_failed: false,
            lost_shown: 0,
            take_plan: None,
            ui: UiState::default(),
            status: String::new(),
            services,
            autosave_dir: None,
            autosaver: None,
            autosaved_rev: 0,
            leftovers: Vec::new(),
            panic_rev: 0,
            last_autosave: None,
            main_meter: MeterDisplay::default(),
            meters: HashMap::new(),
            meter_snapshot: soundcraft_playback::MeterSnapshot::default(),
            last_meter_poll: f64::NEG_INFINITY,
            sim: None,
            record_hint: None,
            quit_requested: false,
            frame_ms: 0.0,
            last_frame: None,
            last_rev: 0,
            control_rx: None,
            pending_shots: Vec::new(),
            synthetic: Vec::new(),
            synthetic_grace: 0,
            synthetic_mods: None,
            theme_applied: false,
            dialog: None,
            editors: Vec::new(),
            arrange: arrange::ArrangeState::default(),
            copied: Vec::new(),
            mixer: mixer::MixerState::default(),
        }
    }

    pub fn with_control(mut self, rx: Receiver<ControlRequest>) -> Self {
        self.control_rx = Some(rx);
        self
    }

    /// The edit cursor: where Play starts and Paste lands. Distinct from the selection
    /// (`edit.selection`); view state in [`UiState`], never in the document.
    pub fn cursor(&self) -> Samples {
        self.ui.cursor.max(0)
    }

    /// The track the cursor was last set on (paste target), if it still exists.
    pub fn cursor_track(&self) -> Option<TrackId> {
        let id = self.ui.cursor_track?;
        self.engine.session().track(id).map(|t| t.id)
    }

    /// Set the cursor (and its track). View state only: the document is untouched.
    pub fn set_cursor(&mut self, at: Samples, track: Option<TrackId>) {
        self.ui.cursor = at.max(0);
        if let Some(t) = track {
            self.ui.cursor_track = Some(t);
        }
        if !self.engine.transport.playing {
            self.engine.transport.position = self.ui.cursor;
        }
    }

    /// Plan 06, L7: this UI offers neither Shuffle nor Spot, so edit commands run as Slip while the
    /// document keeps the mode it was saved with. Only the commands see the override.
    fn slip_override(&mut self, id: &str) -> bool {
        use soundcraft_model::EditMode;
        let edits = id.starts_with("edit.") || id.starts_with("clip.") || id.starts_with("midi.");
        if !edits || matches!(id, "edit.mode" | "edit.undo" | "edit.redo") {
            return false;
        }
        if matches!(self.engine.session().edit.edit_mode, EditMode::Shuffle | EditMode::Spot) {
            self.engine.session_mut().edit.edit_mode = EditMode::Slip;
            return true;
        }
        false
    }

    /// Run a command; errors go to the status line. Plug-in state is captured before saves.
    pub fn run(&mut self, id: &str, params: Value) -> Result<Value, String> {
        if id == "ui.set" {
            return self.set_ui(&params);
        }
        if id.starts_with("session.save") || id.starts_with("file.bounce") {
            // Third-party plugins keep state the session can't see: store it first.
            self.capture_plugin_states();
        }
        let replacing = matches!(id, "session.new" | "session.open" | "session.close" | "session.revert");
        let (restore, mode) = {
            let m = self.engine.session().edit.edit_mode;
            (self.slip_override(id), m)
        };
        let result = self.engine.execute(id, &params);
        if restore && self.engine.session().edit.edit_mode == soundcraft_model::EditMode::Slip {
            self.engine.session_mut().edit.edit_mode = mode;
        }
        match result {
            Ok(v) => {
                if replacing || (id.starts_with("session.save") && !self.engine.is_dirty()) {
                    self.release_autosave(false);
                }
                if id == "app.quit" {
                    self.quit_requested = true;
                }
                Ok(v)
            }
            Err(e) => {
                self.status = e.to_string();
                log::warn!("{id}: {e}");
                Err(e.to_string())
            }
        }
    }

    /// `ui.set`: patch [`UiState`] fields. Plug-in folders are not changed this way.
    fn set_ui(&mut self, p: &Value) -> Result<Value, String> {
        let mut cur = serde_json::to_value(&self.ui).map_err(|e| e.to_string())?;
        if let (Some(obj), Some(patch)) = (cur.as_object_mut(), p.as_object()) {
            for (k, v) in patch {
                obj.insert(k.clone(), v.clone());
            }
        }
        let ns: UiState = serde_json::from_value(cur).map_err(|e| format!("bad ui state: {e}"))?;
        if ns.plugin_dirs != self.ui.plugin_dirs {
            return Err("plugin folders can't be changed over the control channel".into());
        }
        let mut ns = ns;
        (ns.cursor, ns.cursor_track, ns.focus_track) = (self.ui.cursor, self.ui.cursor_track, self.ui.focus_track);
        let before = std::mem::replace(&mut self.ui, ns);
        // Editors no longer listed close; newly listed ones open (retried until loaded).
        for key in before.plugin_windows.iter().filter(|k| !self.ui.plugin_windows.contains(k)) {
            if let Some(p) = &self.player {
                p.close_editor(key.0, key.1);
            }
            self.editors.retain(|e| e.key != *key);
        }
        let wanted: Vec<(TrackId, usize)> = self.ui.plugin_windows.clone();
        for key in wanted {
            self.want_editor(key);
        }
        Ok(json!({"ui": self.ui}))
    }

    /// Ask for a plug-in editor window; it opens as soon as the plug-in is loaded.
    pub fn want_editor(&mut self, key: (TrackId, usize)) {
        if !self.ui.plugin_windows.contains(&key) {
            self.ui.plugin_windows.push(key);
        }
        if self.editors.iter().any(|e| e.key == key) {
            return;
        }
        let now = self.last_frame.unwrap_or_default();
        self.editors.push(EditorWanted { key, deadline: now + 30.0, next_try: now, opened: false });
    }

    /// Open the editors asked for once their plug-ins are loaded; forget the ones the user closed.
    fn drive_editors(&mut self) {
        let now = self.last_frame.unwrap_or_default();
        let Some(p) = &self.player else {
            self.editors.clear();
            return;
        };
        let mut keep = Vec::new();
        let mut closed = Vec::new();
        for mut w in std::mem::take(&mut self.editors) {
            let open = p.editor_open(w.key.0, w.key.1);
            if open {
                w.opened = true;
                keep.push(w);
                continue;
            }
            if w.opened {
                // Closed by the user (or the instance was recreated): no longer wanted.
                closed.push(w.key);
                continue;
            }
            if now >= w.next_try {
                match p.open_editor(w.key.0, w.key.1) {
                    Ok(()) => w.opened = true,
                    Err(e) if now > w.deadline => {
                        self.status = format!("Plug-in editor: {e}");
                        closed.push(w.key);
                        continue;
                    }
                    Err(_) => w.next_try = now + 0.5,
                }
            }
            keep.push(w);
        }
        self.editors = keep;
        self.ui.plugin_windows.retain(|k| !closed.contains(k));
    }

    pub fn is_playing(&self) -> bool {
        self.engine.transport.playing
    }

    /// Current transport position: the playhead while playing, else the cursor.
    pub fn position(&self) -> Samples {
        if self.engine.transport.playing { self.engine.transport.position } else { self.cursor() }
    }

    /// Why the last Record press did nothing, for a few seconds and while it still holds.
    pub fn record_hint(&self) -> Option<&'static str> {
        let (at, why) = self.record_hint?;
        let fresh = self.last_frame.unwrap_or_default() - at < 4.0;
        (fresh && !self.engine.session().tracks.iter().any(|t| t.mixer.record_arm)).then_some(why)
    }

    /// Per-frame logic: control channel, plug-in editors, transport, document sync, autosave.
    pub fn logic(&mut self, ctx: &egui::Context) {
        if ctx.current_pass_index() > 0 {
            return;
        }
        if !self.theme_applied {
            theme::apply(ctx);
            self.theme_applied = true;
        }
        let now = ctx.input(|i| i.time);
        let dt = self.last_frame.replace(now).map_or(1.0 / 60.0, |t| (now - t) as f32).clamp(0.0, 0.25);
        self.frame_ms = self.frame_ms * 0.9 + dt * 1000.0 * 0.1;
        control::drain(self, ctx);
        // E7: plug-in editors only work while `Player::idle` runs every frame.
        self.apply_editor_edits();
        self.drive_editors();
        theme::probe_fonts(ctx);
        let wheeling = self.drive_wheel(ctx, now);
        if !wheeling && !ctx.input(|i| i.pointer.any_down()) && !ctx.egui_wants_keyboard_input() {
            self.engine.end_merge();
        }
        self.watch_output();
        self.watch_input();
        // Stop the old playback before sending a replacement session to the audio thread.
        self.handle_transport(dt);
        if self.engine.revision != self.last_rev {
            self.last_rev = self.engine.revision;
            if let Some(p) = &self.player {
                p.update_session(self.engine.session_arc());
            }
        }
        self.autosave_changes();
        self.autosave();
        if self.recorder.is_none()
            && !self.recorder_failed
            && self.engine.session().tracks.iter().any(|t| t.mixer.input_monitor || t.mixer.record_arm)
        {
            self.ensure_input();
        }
        // Autosave only runs on frames, and plugin editors are native windows egui never hears from.
        // egui takes its predicted frame time off every delay, so add it back.
        let editors = self.player.as_ref().is_some_and(Player::any_editor_open) || !self.editors.is_empty();
        ctx.request_repaint_after_secs(if editors { 0.03 } else { 1.0 } + ctx.input(|i| i.predicted_dt));
        control::collect_screenshots(self, ctx);
    }

    /// Feed queued synthetic input (from the control channel) into egui: one pointer event per
    /// frame (so drags span frames like real ones), keys as press+release pairs, text at once.
    /// Copied from `soundcraft-ui-egui` (`SoundApp::raw_input_hook`, transit 2af868e), plus the
    /// modifiers a synthetic gesture holds (`ui.drag {alt: true}`) applied to the frame's input.
    pub fn raw_input_hook(&mut self, raw: &mut egui::RawInput) {
        if !self.synthetic.is_empty() {
            self.synthetic_grace = 6;
        }
        if self.synthetic_grace > 0 {
            self.synthetic_grace -= 1;
            raw.events.retain(|e| {
                !matches!(e, egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. } | egui::Event::PointerGone | egui::Event::MouseMoved(_))
            });
        }
        let Some(first) = self.synthetic.first().cloned() else {
            if let Some(m) = self.synthetic_mods.take() {
                raw.events.insert(0, egui::Event::ModifiersChanged(m));
            }
            return;
        };
        let take = match first {
            egui::Event::Key { pressed: true, .. } => {
                if matches!(self.synthetic.get(1), Some(egui::Event::Key { pressed: false, .. })) {
                    2
                } else {
                    1
                }
            }
            _ => 1,
        };
        let n = take.min(self.synthetic.len());
        let evs: Vec<egui::Event> = self.synthetic.drain(..n).collect();
        let mut release = false;
        for e in &evs {
            match e {
                egui::Event::PointerButton { pos, pressed, modifiers, .. } => {
                    if *pressed {
                        self.synthetic_mods = Some(*modifiers);
                    } else {
                        release = true;
                    }
                    // Keep the hover position current for widgets that check it.
                    raw.events.push(egui::Event::PointerMoved(*pos));
                }
                egui::Event::PointerMoved(pos) => raw.events.push(egui::Event::PointerMoved(*pos)),
                _ => {}
            }
        }
        if let Some(m) = self.synthetic_mods {
            raw.events.insert(0, egui::Event::ModifiersChanged(m));
        }
        if release {
            // Held through this frame's release, then let go on the next.
            self.synthetic_mods = Some(egui::Modifiers::NONE);
        }
        raw.events.extend(evs.into_iter().filter(|e| !matches!(e, egui::Event::PointerMoved(_))));
    }

    /// Lay out the whole window.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.set_style(ctx.global_style());
        menus::shortcuts(self, &ctx);
        menus::menu_bar(self, ui);
        transport::bar(self, ui);
        menus::status_bar(self, ui);
        mixer::panes(self, ui);
        if self.ui.mixer_full {
            mixer::show_full(self, ui);
        } else {
            arrange::show(self, ui);
        }
        menus::dialogs(self, &ctx);
        menus::recovery(self, &ctx);
        if self.is_playing() || self.engine.transport.recording {
            ctx.request_repaint_after(std::time::Duration::from_millis(1000 / 30));
        } else if self.main_meter.level[0] > 0.0001 {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        } else if !self.pending_shots.is_empty() || !self.synthetic.is_empty() {
            ctx.request_repaint();
        }
    }

    /// UI state as JSON (control channel `ui.inspect`).
    pub fn inspect(&self, ctx: &egui::Context) -> Value {
        let size = ctx.content_rect().size();
        let open: Vec<(TrackId, usize)> =
            self.player.as_ref().map(|p| self.ui.plugin_windows.iter().copied().filter(|(t, s)| p.editor_open(*t, *s)).collect()).unwrap_or_default();
        json!({
            "ui": self.ui,
            "window_size": [size.x, size.y],
            "playing": self.is_playing(),
            "recording": self.engine.transport.recording,
            "input_open": self.recorder.is_some(),
            "position": self.position(),
            "cursor": self.cursor(),
            "cursor_track": self.cursor_track(),
            "selected_clips": self.engine.session().edit.selected_clips,
            "dialog": self.dialog.as_ref().map(|Dialog::SaveChanges { .. }| "save_changes"),
            "frame_ms": self.frame_ms,
            "audio_device": self.player.as_ref().map(|p| p.device_name.clone()),
            "editors_open": open,
            "main_meter": self.main_meter.level,
            "status": self.status,
            "layout": self.arrange.layout,
            "focus_track": self.focus_track(),
            "mixer": self.mixer.layout,
            "devices": self.mixer.devices,
            "track_menu": self.arrange.menu,
            "menu_state": {"filter_rect": self.mixer.filter_rect},
            "readout_edit": self.mixer.edit.as_ref().map(|e| json!({
                "control": format!("{:?}", e.target),
                "text": e.text,
                "rect": self.mixer.edit_rect,
            })),
            "wheel_merge": self.mixer.wheel.is_some(),
            "plugin_scan": plugin_scan::inspect(),
        })
    }
}

#[cfg(test)]
mod tests;
