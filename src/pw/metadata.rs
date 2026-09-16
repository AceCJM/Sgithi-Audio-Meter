//! Helpers for the `default.audio.sink`/`default.audio.source` metadata keys, whose values are
//! small JSON objects like `{"name":"alsa_output.pci-0000_00_1f.3.analog-stereo"}`.

pub const DEFAULT_SINK_KEY: &str = "default.audio.sink";
pub const DEFAULT_SOURCE_KEY: &str = "default.audio.source";
pub const JSON_TYPE: &str = "Spa:String:JSON";

pub fn encode_default_value(node_name: &str) -> String {
    serde_json::json!({ "name": node_name }).to_string()
}

pub fn decode_default_name(value: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(value).ok()?;
    parsed.get("name")?.as_str().map(str::to_owned)
}
