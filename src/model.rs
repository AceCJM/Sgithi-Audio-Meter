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

#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub id: u32,
    pub name: String,
    pub description: Option<String>,
    pub media_class: String,
    pub volumes: Vec<f32>,
    pub mute: bool,
}

impl NodeInfo {
    pub fn group(&self) -> MixerGroup {
        classify(&self.media_class)
    }

    pub fn is_hardware(&self) -> bool {
        is_hardware(&self.media_class)
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
                    NodeInfo { id, name, description, media_class, volumes: Vec::new(), mute: false },
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
        }
    }

    fn find_node_by_name(&self, name: &str) -> Option<u32> {
        self.nodes.values().find(|n| n.name == name).map(|n| n.id)
    }
}
