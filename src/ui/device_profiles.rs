//! The user's last-chosen profile per device (e.g. picking "Off" or a specific output mode for a
//! multi-profile card), saved by device *name* - like `ui::overrides`, ids aren't stable across a
//! process restart, and for a Bluetooth device not even across a disconnect/reconnect, since it
//! gets a brand-new PipeWire global id each time it reconnects. Saved to
//! `~/.config/sgithi-audio-meter/device-profiles.json`.
//!
//! Matched back up by profile *description* rather than PipeWire's numeric index: nothing
//! guarantees a profile's index is the same across separate enumerations (a Bluetooth device's
//! available profile set in particular depends on which codecs happen to be negotiated at connect
//! time), but the description text ("Off", "High Fidelity Playback (A2DP Sink)", ...) is stable
//! for the same profile.

use std::collections::HashMap;
use std::path::PathBuf;

pub struct DeviceProfiles {
    by_name: HashMap<String, String>,
}

fn config_path() -> Option<PathBuf> {
    let base = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg)
    } else {
        PathBuf::from(std::env::var("HOME").ok()?).join(".config")
    };
    Some(base.join("sgithi-audio-meter").join("device-profiles.json"))
}

impl DeviceProfiles {
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
                    log::warn!("failed to save device profiles to {}: {e}", path.display());
                }
            }
            Err(e) => log::warn!("failed to serialize device profiles: {e}"),
        }
    }

    pub fn saved(&self, device_name: &str) -> Option<&str> {
        self.by_name.get(device_name).map(String::as_str)
    }

    /// Remember `description` as the chosen profile for `device_name` and persist immediately.
    pub fn set(&mut self, device_name: &str, description: &str) {
        self.by_name.insert(device_name.to_string(), description.to_string());
        self.save();
    }
}
