//! Plug-in scan bootstrap. **Copied** from `crates/ui-egui/src/plugin_folders.rs` (`PluginDirs`,
//! `start`, `scan_child`) at `transit` 2af868e (file last changed da11565). The app must answer
//! [`scan_child`] first thing in `main`, or every scan child starts a whole app (E1/E2).

use std::path::{Path, PathBuf};

/// The user's own plugin folders, scanned besides the standard ones (kept with the preferences).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PluginDirs {
    pub vst3: Vec<String>,
    pub clap: Vec<String>,
}

/// At launch: remembers scans in `settings_dir` (when there is one), adds the user's folders and
/// starts scanning on a background thread. Plugin files are loaded for the scan by `scanner`
/// (this app's own program, run with [`scan_child`]'s arguments) in a child process.
pub fn start(dirs: &PluginDirs, settings_dir: Option<&Path>, scanner: Option<PathBuf>) {
    soundcraft_vst3_host::set_extra_dirs(dirs.vst3.iter().map(PathBuf::from).collect());
    soundcraft_clap_host::set_extra_dirs(dirs.clap.iter().map(PathBuf::from).collect());
    soundcraft_vst3_host::set_scanner(scanner.clone());
    soundcraft_clap_host::set_scanner(scanner);
    if let Some(dir) = settings_dir {
        soundcraft_vst3_host::set_cache_file(Some(dir.join("plugins-vst3.json")));
        soundcraft_clap_host::set_cache_file(Some(dir.join("plugins-clap.json")));
        soundcraft_vst3_host::scan_in_background();
        soundcraft_clap_host::scan_in_background();
    }
}

/// The scan child process's work: with `<flag> <file>` arguments (see [`start`]), loads that one
/// plugin file and returns the line to print; `None` for any other arguments.
pub fn scan_child(args: &[String]) -> Option<String> {
    let [flag, file] = args else { return None };
    let file = Path::new(file);
    if flag == soundcraft_vst3_host::SCAN_FLAG {
        Some(soundcraft_vst3_host::scan_reply(file))
    } else if flag == soundcraft_clap_host::SCAN_FLAG {
        Some(soundcraft_clap_host::scan_reply(file))
    } else {
        None
    }
}
