//! Saves/restores patchbay node positions and hidden-node state across restarts, keyed by node
//! *name* (PipeWire object ids aren't stable across restarts, but `node.name` - e.g.
//! `alsa_output.usb-...` - generally is for the same physical device/stream type).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

fn config_dir() -> Option<PathBuf> {
    let base = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg)
    } else {
        PathBuf::from(std::env::var("HOME").ok()?).join(".config")
    };
    Some(base.join("sgithi-audio-meter"))
}

fn layout_path() -> Option<PathBuf> {
    Some(config_dir()?.join("patchbay-layout.json"))
}

fn hidden_path() -> Option<PathBuf> {
    Some(config_dir()?.join("patchbay-hidden.json"))
}

fn read_json<T: serde::de::DeserializeOwned + Default>(path: Option<PathBuf>) -> T {
    let Some(path) = path else { return T::default() };
    let Ok(contents) = std::fs::read_to_string(&path) else { return T::default() };
    serde_json::from_str(&contents).unwrap_or_default()
}

fn write_json<T: serde::Serialize>(path: Option<PathBuf>, what: &str, value: &T) {
    let Some(path) = path else { return };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("failed to create config dir {}: {e}", parent.display());
            return;
        }
    }
    match serde_json::to_string_pretty(value) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                log::warn!("failed to save {what} to {}: {e}", path.display());
            }
        }
        Err(e) => log::warn!("failed to serialize {what}: {e}"),
    }
}

/// Load saved positions, if any. Missing file, unreadable file, or malformed JSON are all
/// treated as "nothing saved yet" rather than an error - this is a convenience cache, not a
/// source of truth the app depends on to function.
pub fn load() -> HashMap<String, (f64, f64)> {
    read_json(layout_path())
}

/// Save the current positions, keyed by node name. Best-effort: a failure to write (e.g. no
/// writable home directory) is logged and otherwise ignored, since losing the saved layout is
/// not fatal to the app - it just falls back to the auto-layout heuristic next launch.
pub fn save(positions: &HashMap<String, (f64, f64)>) {
    write_json(layout_path(), "patchbay layout", positions);
}

/// Load the set of node names hidden from the patchbay canvas. Same best-effort semantics as
/// `load()`.
pub fn load_hidden() -> HashSet<String> {
    read_json(hidden_path())
}

pub fn save_hidden(hidden: &HashSet<String>) {
    write_json(hidden_path(), "patchbay hidden-node list", hidden);
}
