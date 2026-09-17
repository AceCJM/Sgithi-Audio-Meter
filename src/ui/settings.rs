//! Persisted app-wide UI settings - currently just the fader ceiling. Separate from
//! `ui::overrides` (per-node display name/category, keyed by node) since this applies globally.
//! Saved to `~/.config/sgithi-audio-meter/settings.json`, same pattern as `ui::overrides` and
//! `ui::patchbay::persistence`.

use std::path::PathBuf;

/// pavucontrol's own over-amplification ceiling (150%) - this app's fader range (see
/// `ui::strip`) before any Settings change.
pub const DEFAULT_FADER_MAX: f32 = 1.5;
/// Never let the ceiling drop below unity gain.
pub const MIN_FADER_MAX: f32 = 1.0;
/// Generous upper bound, mostly to give the Settings control a fixed range.
pub const MAX_FADER_MAX: f32 = 3.0;

pub struct Settings {
    pub fader_max: f32,
}

fn config_path() -> Option<PathBuf> {
    let base = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg)
    } else {
        PathBuf::from(std::env::var("HOME").ok()?).join(".config")
    };
    Some(base.join("sgithi-audio-meter").join("settings.json"))
}

impl Settings {
    /// Missing file, unreadable file, or malformed JSON are all treated as "nothing saved yet"
    /// rather than an error - this is a convenience cache, not a source of truth the app depends
    /// on to function.
    pub fn load() -> Self {
        let fader_max = config_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|contents| serde_json::from_str::<f32>(&contents).ok())
            .map(|max| max.clamp(MIN_FADER_MAX, MAX_FADER_MAX))
            .unwrap_or(DEFAULT_FADER_MAX);
        Self { fader_max }
    }

    /// Best-effort: a failure to write (e.g. no writable home directory) is logged and otherwise
    /// ignored, since losing a saved setting just falls back to the default next launch.
    fn save(&self) {
        let Some(path) = config_path() else { return };
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                log::warn!("failed to create config dir {}: {e}", parent.display());
                return;
            }
        }
        match serde_json::to_string_pretty(&self.fader_max) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    log::warn!("failed to save settings to {}: {e}", path.display());
                }
            }
            Err(e) => log::warn!("failed to serialize settings: {e}"),
        }
    }

    /// Set (and persist) a new fader ceiling, clamped to `MIN_FADER_MAX..=MAX_FADER_MAX`.
    pub fn set_fader_max(&mut self, fader_max: f32) {
        self.fader_max = fader_max.clamp(MIN_FADER_MAX, MAX_FADER_MAX);
        self.save();
    }

    pub fn reset_to_default(&mut self) {
        self.set_fader_max(DEFAULT_FADER_MAX);
    }
}
