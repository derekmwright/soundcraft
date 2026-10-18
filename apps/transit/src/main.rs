//! transitDAW desktop app (working name): the studio UI over the SoundCraft engine.
//!
//! Usage: `transit [--demo] [--control <port>] [--no-audio] [session.scraft]`
//!
//! `--control <port>` (or `TRANSIT_CONTROL_PORT`) starts a localhost JSON-lines control server
//! (`0` picks a free port and prints it). See `soundcraft_ui_studio::control` for the methods.
//!
//! Structure copied from `apps/soundcraft/src/main.rs` at `transit` 2af868e (file last changed
//! 04e1fe8): the scan child answered first in `main`, the logger, prefs, `--control`, `--no-audio`,
//! `--demo`, and the exit order in `on_exit` (E5: `close_input` before the player is dropped).
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod config;
mod control_server;
#[cfg(any(target_os = "windows", test))]
mod graphics;
mod logging;

use soundcraft_engine::Engine;
use soundcraft_ui_studio::{Services, StudioApp, UiState};
use std::sync::Arc;

struct App(StudioApp, Option<&'static logging::AppLogger>);

/// Sends the title only when it changes (copied from `apps/soundcraft`).
fn set_title(ctx: &egui::Context, title: String) {
    let id = egui::Id::new("window_title");
    if ctx.data(|d| d.get_temp::<String>(id)).as_deref() != Some(title.as_str()) {
        ctx.data_mut(|d| d.insert_temp(id, title.clone()));
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // What the realtime audio thread logged is written here, off that thread.
        if let Some(logger) = self.1 {
            logger.report_audio_thread();
        }
        self.0.guarded(|app| app.logic(ctx));
        let dropped: Vec<String> =
            ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_string_lossy().into_owned()).filter(|s| !s.is_empty()).collect());
        for p in dropped {
            open_path(&mut self.0, &p);
        }
        if self.0.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if ctx.input(|i| i.viewport().close_requested()) && self.0.engine.is_dirty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.0.run_or_ask("app.quit", serde_json::json!({}));
        }
        let (inner, maximized) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().maximized.unwrap_or(false)));
        self.0.ui.window_maximized = maximized;
        if let Some(r) = inner.filter(|_| !maximized) {
            self.0.ui.window_size = (r.size() * ctx.zoom_factor()).round().into();
        }
        let dirty = if self.0.engine.is_dirty() { " *" } else { "" };
        set_title(ctx, format!("{}{dirty} — transitDAW", self.0.engine.session().name));
    }
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.0.raw_input_hook(raw);
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.guarded(|app| app.ui(ui));
    }
    fn on_exit(&mut self) {
        self.0.close_autosave();
        save_prefs(&self.0.ui);
        if let Some(logger) = self.1 {
            logger.report_audio_thread();
        }
        // E5 quit order (copied from `apps/soundcraft`): finish any take and hand the input back
        // to the player, then release the player's plugin instances, then shut plugin hosting down.
        self.0.close_input();
        self.0.player = None;
        soundcraft_engine::shutdown_plugin_hosts();
    }
}

fn open_path(app: &mut StudioApp, p: &str) {
    let lower = p.to_ascii_lowercase();
    if lower.ends_with(".scraft") {
        app.run_or_ask("session.open", serde_json::json!({"path": p}));
        return;
    }
    let r = if lower.ends_with(".mid") || lower.ends_with(".midi") || lower.ends_with(".smf") {
        app.run("file.import_midi", serde_json::json!({"path": p}))
    } else {
        let at = app.cursor();
        let mut params = serde_json::json!({"path": p, "at": at});
        if let Some(t) = app.cursor_track() {
            params["track"] = serde_json::json!(t.0);
        }
        app.run("file.import_audio", params)
    };
    if let Err(e) = r {
        log::warn!("{p}: {e}");
    }
}

fn prefs_path() -> Option<std::path::PathBuf> {
    config::config_dir().map(|b| b.join("ui.json"))
}

/// Runs without preferences (`TRANSIT_NO_PREFS` or `SOUNDCRAFT_NO_PREFS`, agents' test runs)
/// neither read nor write them, keep no scan cache and write no log file.
fn prefs_enabled() -> bool {
    std::env::var_os("TRANSIT_NO_PREFS").is_none() && std::env::var_os("SOUNDCRAFT_NO_PREFS").is_none()
}

fn load_prefs() -> Option<UiState> {
    if !prefs_enabled() {
        return None;
    }
    match config::load_ui(&prefs_path()?) {
        Ok(state) => state,
        Err(error) => {
            log::warn!("{error}");
            None
        }
    }
}

fn save_prefs(ui: &UiState) {
    if !prefs_enabled() {
        return;
    }
    if let Some(path) = prefs_path()
        && let Err(error) = config::save_ui(&path, ui)
    {
        log::warn!("{error}");
    }
}

fn services() -> Services {
    Services {
        pick_open: Some(Box::new(|purpose: &str| {
            let d = rfd::FileDialog::new();
            let d = match purpose {
                "session.open" => d.add_filter("Session", &["scraft"]),
                _ => d.add_filter("Audio", &["wav", "wave", "aif", "aiff", "flac", "mp3", "ogg"]),
            };
            d.pick_file().map(|p| p.to_string_lossy().into_owned())
        })),
        pick_save: Some(Box::new(|default: &str| {
            rfd::FileDialog::new().add_filter("Session", &["scraft"]).set_file_name(default).save_file().map(|p| p.to_string_lossy().into_owned())
        })),
    }
}

/// A saved window size, or the default when the prefs file holds nonsense.
fn sane_window_size([w, h]: [f32; 2]) -> [f32; 2] {
    if w.is_finite() && h.is_finite() && w >= 960.0 && h >= 600.0 { [w.min(16_384.0), h.min(16_384.0)] } else { [1600.0, 1000.0] }
}

fn main() -> eframe::Result {
    // A plugin scan's child process: load one file, answer, exit. First, before anything else
    // (E1/E2: otherwise every scan child starts a whole app).
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(reply) = soundcraft_ui_studio::plugin_scan::scan_child(&args) {
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), "{reply}");
        let _ = std::io::stdout().flush();
        // Run the module exits now, from `main`, not from the exit handler: a plugin whose
        // `ExitDll` waits for its own threads deadlocks inside `exit()`.
        soundcraft_engine::shutdown_plugin_hosts();
        return Ok(());
    }
    let logger = logging::install();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("transit {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let demo = args.iter().any(|a| a == "--demo" || a == "--sample");
    let no_audio = args.iter().any(|a| a == "--no-audio");
    let control_port: Option<u16> = args
        .iter()
        .position(|a| a == "--control")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse().ok())
        .or_else(|| std::env::var("TRANSIT_CONTROL_PORT").ok().and_then(|p| p.parse().ok()));
    let files: Vec<String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| !(a.starts_with("--") || (i > &0 && args.get(i - 1).is_some_and(|p| p == "--control"))))
        .map(|(_, a)| a.clone())
        .collect();
    log::info!("transit {} ({} {})", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
    if let Some(logger) = logger {
        match config::config_dir() {
            _ if !prefs_enabled() => logger.no_file(),
            Some(dir) => match logger.attach_dir(&dir.join(logging::LOG_DIR)) {
                Ok(path) => log::info!("log file {}", path.display()),
                Err(e) => log::warn!("no log file: {e}"),
            },
            None => {
                logger.no_file();
                log::warn!("no log file: no settings directory");
            }
        }
    }

    let engine = if demo { soundcraft_engine::demo::demo_engine() } else { Engine::default() };
    let prefs = load_prefs().unwrap_or_default();
    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("transitDAW")
            .with_inner_size(sane_window_size(prefs.window_size))
            .with_maximized(prefs.window_maximized)
            .with_min_inner_size([960.0, 600.0])
            .with_drag_and_drop(true)
            .with_app_id("transitdaw"),
        ..Default::default()
    };
    // Before eframe creates the wgpu instance: default Windows to DirectX 12 with FXC (see graphics.rs).
    #[cfg(target_os = "windows")]
    graphics::configure(&mut options, eframe::wgpu::Backends::from_env(), eframe::wgpu::Dx12Compiler::from_env());
    eframe::run_native(
        "transitDAW",
        options,
        Box::new(move |cc| {
            let player =
                if no_audio { None } else { Some(soundcraft_playback::Player::with_config(Arc::new(engine.session().clone()), &prefs.audio)) };
            let mut app = StudioApp::new(engine, player, services());
            // Our own autosave folder: the two apps must not recover each other's sessions.
            app.autosave_dir = prefs_path().and_then(|p| p.parent().map(|d| d.join("Autosave")));
            app.leftovers = app.autosave_dir.as_deref().map(soundcraft_engine::autosave::leftovers).unwrap_or_default();
            app.ui = prefs;
            // The plug-in scan cache is SoundCraft's, shared; runs without preferences keep none.
            let cache = config::soundcraft_dir().filter(|_| prefs_enabled());
            soundcraft_ui_studio::plugin_scan::start(&app.ui.plugin_dirs, cache.as_deref(), std::env::current_exe().ok());
            if let Some(port) = control_port {
                let rx = control_server::start(port, cc.egui_ctx.clone());
                app = app.with_control(rx);
            }
            for f in &files {
                open_path(&mut app, f);
            }
            Ok(Box::new(App(app, logger)))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::sane_window_size;

    #[test]
    fn nonsense_window_sizes_fall_back() {
        assert_eq!(sane_window_size([f32::NAN, 10.0]), [1600.0, 1000.0]);
        assert_eq!(sane_window_size([1200.0, 800.0]), [1200.0, 800.0]);
    }
}
