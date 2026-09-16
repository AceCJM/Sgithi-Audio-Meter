//! Commands flowing from the UI thread to the PipeWire thread, sent over a
//! [`pipewire::channel`] attached to the PipeWire main loop.

#[derive(Debug, Clone)]
pub enum Command {
    SetVolume { node_id: u32, volumes: Vec<f32> },
    SetMute { node_id: u32, mute: bool },
    CreateLink { output_node: u32, output_port: u32, input_node: u32, input_port: u32 },
    DestroyLink { link_id: u32 },
    SetDefaultSink { node_name: String },
    SetDefaultSource { node_name: String },
    SetProfile { device_id: u32, profile_index: i32 },
    Terminate,
}
