//! Persisted app-wide UI settings: the fader ceiling and the color theme. Separate from
//! `ui::overrides` (per-node display name/category, keyed by node) since these apply globally.
//! Saved to `~/.config/sgithi-audio-meter/settings.json`, same pattern as `ui::overrides` and
//! `ui::patchbay::persistence`. Deliberately has no dependency on `gtk`/`adw` - `ui::app` maps
//! `Theme` to `adw::ColorScheme`, keeping this module plain data + persistence like `overrides.rs`.

use std::path::PathBuf;

/// pavucontrol's own over-amplification ceiling (150%) - this app's fader range (see
/// `ui::strip`) before any Settings change.
pub const DEFAULT_FADER_MAX: f32 = 1.5;
/// Never let the ceiling drop below unity gain.
pub const MIN_FADER_MAX: f32 = 1.0;
/// Generous upper bound, mostly to give the Settings control a fixed range.
pub const MAX_FADER_MAX: f32 = 3.0;

/// Light, to match the user's "brighter" ask for the libadwaita migration.
pub const DEFAULT_THEME: Theme = Theme::Light;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    System,
    Light,
    Dark,
}

impl Theme {
    /// All variants, in the order the Settings dialog's theme selector lists them.
    pub const ALL: [Theme; 3] = [Theme::System, Theme::Light, Theme::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Theme::System => "System",
            Theme::Light => "Light",
            Theme::Dark => "Dark",
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "system" => Some(Theme::System),
            "light" => Some(Theme::Light),
            "dark" => Some(Theme::Dark),
            _ => None,
        }
    }
}

pub struct Settings {
    pub fader_max: f32,
    pub theme: Theme,
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
    /// on to function. Also accepts the old format (this file used to be just a bare fader_max
    /// float, before the theme setting existed) by falling back to the default theme when
    /// `theme` isn't present.
    pub fn load() -> Self {
        let parsed = config_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|contents| serde_json::from_str::<serde_json::Value>(&contents).ok());

        let fader_max = parsed
            .as_ref()
            .and_then(|v| v.get("fader_max").or(Some(v)))
            .and_then(|v| v.as_f64())
            .map(|max| (max as f32).clamp(MIN_FADER_MAX, MAX_FADER_MAX))
            .unwrap_or(DEFAULT_FADER_MAX);
        let theme = parsed
            .as_ref()
            .and_then(|v| v.get("theme"))
            .and_then(|v| v.as_str())
            .and_then(Theme::from_str)
            .unwrap_or(DEFAULT_THEME);

        Self { fader_max, theme }
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
        let value = serde_json::json!({ "fader_max": self.fader_max, "theme": self.theme.as_str() });
        match serde_json::to_string_pretty(&value) {
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

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.save();
    }
}
