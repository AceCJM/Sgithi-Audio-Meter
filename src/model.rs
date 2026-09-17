//! Plain, PipeWire- and GTK-agnostic in-memory representation of the audio graph.
//!
//! Nothing here touches `pipewire` or `gtk4` types. The `pw` module builds `Event`s from
//! PipeWire callbacks; the `ui` module applies those events to a `Graph` and reads it to
//! render widgets.

use std::collections::HashMap;

use crate::pw::Event;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Input,
    Output,
}

/// Which mixer column a node belongs on, per the app's Voicemeeter-style unified layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixerGroup {
    /// Hardware capture devices (Audio/Source) and application playback streams
    /// (Stream/Output/Audio, pavucontrol's "Playback" tab).
    Inputs,
    /// Hardware playback devices (Audio/Sink) and application recording streams
    /// (Stream/Input/Audio, pavucontrol's "Recording" tab).
    Outputs,
    /// Anything else (e.g. video streams) - not shown in the mixer.
    Other,
}

/// Classify a node's mixer column from its `media.class` property.
pub fn classify(media_class: &str) -> MixerGroup {
    match media_class {
        "Audio/Source" | "Stream/Output/Audio" => MixerGroup::Inputs,
        "Audio/Sink" | "Stream/Input/Audio" => MixerGroup::Outputs,
        _ => MixerGroup::Other,
    }
}

/// True for `Audio/Sink`/`Audio/Source` nodes backed by real hardware, as opposed to an
/// application's playback/recording stream.
pub fn is_hardware(media_class: &str) -> bool {
    matches!(media_class, "Audio/Sink" | "Audio/Source")
}

/// True for nodes that *receive* audio (hardware `Audio/Sink` outputs, and app
/// `Stream/Input/Audio` recording streams) as opposed to nodes that *produce* it. Peak metering
/// (`pw::peak`) needs this to decide whether to tap a node's monitor (`STREAM_CAPTURE_SINK`) or
/// connect to it directly.
pub fn is_sink_like(media_class: &str) -> bool {
    matches!(media_class, "Audio/Sink" | "Stream/Input/Audio")
}

#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub id: u32,
    pub name: String,
    pub description: Option<String>,
    pub media_class: String,
    pub volumes: Vec<f32>,
    pub mute: bool,
    /// Most recent linear peak sample seen by this node's metering stream (`pw::peak`), 0.0 if
    /// none has arrived yet (e.g. metering hasn't started, or nothing is playing).
    pub peak: f32,
    /// A hardware capture node's `port.type` Route info key (e.g. `"mic"`, `"line"`,
    /// `"headset-mic"`), when known - see `is_mic_like()`.
    pub port_type: Option<String>,
}

impl NodeInfo {
    pub fn group(&self) -> MixerGroup {
        classify(&self.media_class)
    }

    pub fn is_hardware(&self) -> bool {
        is_hardware(&self.media_class)
    }

    /// Classification of a hardware `Audio/Source` as a microphone versus another kind of capture
    /// input (line-in, a device's own monitor/loopback input, etc.).
    ///
    /// Prefers the Route's `port.type` info key (`"mic"`, `"headset-mic"`, `"line"`, ... - parsed
    /// by `pw::device_route`, threaded onto this node via `Event::NodePortTypeChanged`), the
    /// precise signal PipeWire itself uses. Falls back to checking for "mic"/"microphone" in the
    /// node's display name when `port_type` isn't known yet (e.g. right after discovery, before
    /// the owning Device's Route param has arrived) - a heuristic that happened to track the real
    /// distinction for the hardware this was first tested against (a Focusrite Scarlett Solo:
    /// "Input 2 Mic" vs "Monitor Input 3/4", "Input 1 Inst/Line"), but isn't a guarantee.
    pub fn is_mic_like(&self) -> bool {
        if !self.is_hardware() || self.media_class != "Audio/Source" {
            return false;
        }
        match &self.port_type {
            Some(port_type) => port_type.contains("mic"),
            None => self.display_name().to_lowercase().contains("mic"),
        }
    }

    /// Average of the per-channel volumes, for a single-fader display.
    pub fn volume(&self) -> f32 {
        if self.volumes.is_empty() {
            0.0
        } else {
            self.volumes.iter().sum::<f32>() / self.volumes.len() as f32
        }
    }

    pub fn display_name(&self) -> &str {
        self.description.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone)]
pub struct PortInfo {
    pub id: u32,
    pub node_id: u32,
    pub name: String,
    pub direction: Direction,
}

/// One selectable option for a `Device`'s active profile (e.g. "Off", "Analog Stereo Duplex",
/// "Pro Audio" for an audio interface with multiple operating modes).
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileOption {
    pub index: i32,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub id: u32,
    pub name: String,
    pub profiles: Vec<ProfileOption>,
    pub active_profile: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct LinkInfo {
    pub id: u32,
    // Kept for completeness / future use (e.g. resolving a link's endpoint nodes without a port
    // lookup); current UI code resolves endpoints via `output_port`/`input_port` instead.
    #[allow(dead_code)]
    pub output_node: u32,
    pub output_port: u32,
    #[allow(dead_code)]
    pub input_node: u32,
    pub input_port: u32,
}

/// The full known state of the PipeWire graph, as reconstructed on the UI thread from events.
#[derive(Debug, Default)]
pub struct Graph {
    pub nodes: HashMap<u32, NodeInfo>,
    pub ports: HashMap<u32, PortInfo>,
    pub links: HashMap<u32, LinkInfo>,
    pub devices: HashMap<u32, DeviceInfo>,
    pub default_sink: Option<u32>,
    pub default_source: Option<u32>,
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ports_for_node(&self, node_id: u32) -> impl Iterator<Item = &PortInfo> {
        self.ports.values().filter(move |p| p.node_id == node_id)
    }

    /// Apply an `Event` from the PipeWire thread, mutating the graph in place.
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::NodeAdded { id, name, description, media_class } => {
                self.nodes.insert(
                    id,
                    NodeInfo {
                        id,
                        name,
                        description,
                        media_class,
                        volumes: Vec::new(),
                        mute: false,
                        peak: 0.0,
                        port_type: None,
                    },
                );
            }
            Event::NodeRemoved { id } => {
                self.nodes.remove(&id);
            }
            Event::NodeVolumeChanged { id, volumes, mute } => {
                if let Some(node) = self.nodes.get_mut(&id) {
                    node.volumes = volumes;
                    node.mute = mute;
                }
            }
            Event::PortAdded { id, node_id, name, direction } => {
                self.ports.insert(id, PortInfo { id, node_id, name, direction });
            }
            Event::PortRemoved { id } => {
                self.ports.remove(&id);
            }
            Event::LinkAdded { id, output_node, output_port, input_node, input_port } => {
                self.links
                    .insert(id, LinkInfo { id, output_node, output_port, input_node, input_port });
            }
            Event::LinkRemoved { id } => {
                self.links.remove(&id);
            }
            Event::DefaultSinkChanged { node_name } => {
                self.default_sink = node_name.and_then(|name| self.find_node_by_name(&name));
            }
            Event::DefaultSourceChanged { node_name } => {
                self.default_source = node_name.and_then(|name| self.find_node_by_name(&name));
            }
            Event::DeviceAdded { id, name } => {
                self.devices.insert(id, DeviceInfo { id, name, profiles: Vec::new(), active_profile: None });
            }
            Event::DeviceRemoved { id } => {
                self.devices.remove(&id);
            }
            Event::DeviceProfilesUpdated { id, profiles, active } => {
                if let Some(device) = self.devices.get_mut(&id) {
                    device.profiles = profiles;
                    if active.is_some() {
                        device.active_profile = active;
                    }
                }
            }
            Event::PeakLevel { id, peak } => {
                if let Some(node) = self.nodes.get_mut(&id) {
                    node.peak = peak;
                }
            }
            Event::NodePortTypeChanged { id, port_type } => {
                if let Some(node) = self.nodes.get_mut(&id) {
                    node.port_type = port_type;
                }
            }
        }
    }

    fn find_node_by_name(&self, name: &str) -> Option<u32> {
        self.nodes.values().find(|n| n.name == name).map(|n| n.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pw::Event;

    fn node_added(id: u32, media_class: &str) -> Event {
        Event::NodeAdded { id, name: format!("node-{id}"), description: None, media_class: media_class.to_string() }
    }

    #[test]
    fn node_lifecycle() {
        let mut graph = Graph::new();
        graph.apply(node_added(1, "Audio/Sink"));
        assert!(graph.nodes.contains_key(&1));

        graph.apply(Event::NodeVolumeChanged { id: 1, volumes: vec![0.5, 0.5], mute: true });
        let node = &graph.nodes[&1];
        assert_eq!(node.volumes, vec![0.5, 0.5]);
        assert!(node.mute);

        graph.apply(Event::NodeRemoved { id: 1 });
        assert!(!graph.nodes.contains_key(&1));
    }

    #[test]
    fn ports_and_links_lifecycle() {
        let mut graph = Graph::new();
        graph.apply(node_added(1, "Audio/Sink"));
        graph.apply(Event::PortAdded { id: 10, node_id: 1, name: "in".into(), direction: Direction::Input });
        assert_eq!(graph.ports_for_node(1).count(), 1);

        graph.apply(Event::LinkAdded { id: 100, output_node: 2, output_port: 20, input_node: 1, input_port: 10 });
        assert!(graph.links.contains_key(&100));

        graph.apply(Event::LinkRemoved { id: 100 });
        assert!(!graph.links.contains_key(&100));

        graph.apply(Event::PortRemoved { id: 10 });
        assert_eq!(graph.ports_for_node(1).count(), 0);
    }

    #[test]
    fn default_sink_resolves_by_name() {
        let mut graph = Graph::new();
        graph.apply(Event::NodeAdded {
            id: 1,
            name: "alsa_output.pci".into(),
            description: None,
            media_class: "Audio/Sink".into(),
        });
        graph.apply(Event::DefaultSinkChanged { node_name: Some("alsa_output.pci".into()) });
        assert_eq!(graph.default_sink, Some(1));

        graph.apply(Event::DefaultSinkChanged { node_name: None });
        assert_eq!(graph.default_sink, None);

        graph.apply(Event::DefaultSinkChanged { node_name: Some("nonexistent".into()) });
        assert_eq!(graph.default_sink, None);
    }

    #[test]
    fn device_profile_update_preserves_active_when_none() {
        let mut graph = Graph::new();
        graph.apply(Event::DeviceAdded { id: 1, name: "card".into() });
        graph.apply(Event::DeviceProfilesUpdated {
            id: 1,
            profiles: vec![ProfileOption { index: 0, description: "Off".into() }],
            active: Some(0),
        });
        assert_eq!(graph.devices[&1].active_profile, Some(0));

        // A partial EnumProfile update (active: None) must not clobber the already-known
        // active profile - see the `DeviceProfilesUpdated` doc comment on `Event`.
        graph.apply(Event::DeviceProfilesUpdated {
            id: 1,
            profiles: vec![
                ProfileOption { index: 0, description: "Off".into() },
                ProfileOption { index: 1, description: "Analog Stereo".into() },
            ],
            active: None,
        });
        assert_eq!(graph.devices[&1].active_profile, Some(0));
    }

    #[test]
    fn peak_level_and_port_type_update_existing_node() {
        let mut graph = Graph::new();
        graph.apply(node_added(1, "Audio/Source"));

        graph.apply(Event::PeakLevel { id: 1, peak: 0.75 });
        assert_eq!(graph.nodes[&1].peak, 0.75);

        graph.apply(Event::NodePortTypeChanged { id: 1, port_type: Some("mic".into()) });
        assert_eq!(graph.nodes[&1].port_type.as_deref(), Some("mic"));
        assert!(graph.nodes[&1].is_mic_like());
    }
}
