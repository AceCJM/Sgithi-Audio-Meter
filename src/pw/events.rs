//! Events flowing from the PipeWire thread to the UI thread.
//!
//! These are plain, `Send + 'static` values with no PipeWire types inside, so they can safely
//! cross the thread boundary and be applied to `model::Graph` on the GTK thread.

use crate::model::{Direction, ProfileOption};

#[derive(Debug, Clone)]
pub enum Event {
    NodeAdded {
        id: u32,
        name: String,
        description: Option<String>,
        media_class: String,
    },
    NodeRemoved {
        id: u32,
    },
    NodeVolumeChanged {
        id: u32,
        volumes: Vec<f32>,
        mute: bool,
    },
    PortAdded {
        id: u32,
        node_id: u32,
        name: String,
        direction: Direction,
    },
    PortRemoved {
        id: u32,
    },
    LinkAdded {
        id: u32,
        output_node: u32,
        output_port: u32,
        input_node: u32,
        input_port: u32,
    },
    LinkRemoved {
        id: u32,
    },
    DefaultSinkChanged {
        node_name: Option<String>,
    },
    DefaultSourceChanged {
        node_name: Option<String>,
    },
    DeviceAdded {
        id: u32,
        name: String,
    },
    DeviceRemoved {
        id: u32,
    },
    /// `active` is `None` when this update only carries a (possibly partial, as `EnumProfile`
    /// results arrive one at a time) profile list, not a change in which one is active.
    DeviceProfilesUpdated {
        id: u32,
        profiles: Vec<ProfileOption>,
        active: Option<i32>,
    },
}
