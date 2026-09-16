//! User-chosen overrides for a node's display name and (for hardware capture devices) its
//! Microphone/Other Input categorization - both otherwise derived entirely from what PipeWire
//! reports (`NodeInfo::display_name()`/`is_mic_like()`). Saved by node *name* (ids aren't stable
//! across restarts) to `~/.config/sgithi-audio-meter/node-overrides.json`, the same pattern as
//! `ui::patchbay::persistence`.

use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Microphone,
    OtherInput,
}

impl Category {
    fn as_str(self) -> &'static str {
        match self {
            Category::Microphone => "mic",
            Category::OtherInput => "other",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "mic" => Some(Category::Microphone),
            "other" => Some(Category::OtherInput),
            _ => None,
        }
    }
}

/// (custom name, category) - both optional; stored as plain `Option<String>` rather than a
/// custom struct so this round-trips through `serde_json` without needing a `serde` derive
/// dependency (matches how `patchbay::persistence` avoids one too).
type SavedOverride = (Option<String>, Option<String>);

pub struct Overrides {
    by_name: HashMap<String, SavedOverride>,
}

fn config_path() -> Option<PathBuf> {
    let base = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg)
    } else {
        PathBuf::from(std::env::var("HOME").ok()?).join(".config")
    };
    Some(base.join("sgithi-audio-meter").join("node-overrides.json"))
}

impl Overrides {
    pub fn load() -> Self {
        let by_name = config_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|contents| serde_json::from_str(&contents).ok())
            .unwrap_or_default();
        Self { by_name }
    }

    fn save(&self) {
        let Some(path) = config_path() else { return };
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                log::warn!("failed to create config dir {}: {e}", parent.display());
                return;
            }
        }
        match serde_json::to_string_pretty(&self.by_name) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    log::warn!("failed to save node overrides to {}: {e}", path.display());
                }
            }
            Err(e) => log::warn!("failed to serialize node overrides: {e}"),
        }
    }

    pub fn custom_name(&self, node_name: &str) -> Option<&str> {
        self.by_name.get(node_name)?.0.as_deref()
    }

    pub fn category(&self, node_name: &str) -> Option<Category> {
        self.by_name.get(node_name)?.1.as_deref().and_then(Category::from_str)
    }

    /// Set (or, with `None`, clear) both overrides for a node in one go and persist immediately.
    pub fn set(&mut self, node_name: &str, name: Option<String>, category: Option<Category>) {
        if name.is_none() && category.is_none() {
            self.by_name.remove(node_name);
        } else {
            self.by_name.insert(node_name.to_string(), (name, category.map(Category::as_str).map(str::to_string)));
        }
        self.save();
    }
}
