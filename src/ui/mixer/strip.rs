use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{Box as GtkBox, Button, Justification, Label, Orientation, PositionType, Scale, ToggleButton};

use crate::model::{MixerGroup, NodeInfo};
use crate::pw::Command;

/// One vertical fader strip for a single node (hardware device or app stream).
pub struct Strip {
    pub widget: GtkBox,
    pub group: MixerGroup,
    scale: Scale,
    mute_button: ToggleButton,
    default_button: Option<Button>,
    /// Number of channels last reported for this node, so a fader move sets every channel
    /// rather than assuming stereo.
    channels: Rc<Cell<usize>>,
}

impl Strip {
    pub fn new(node: &NodeInfo, cmd_tx: pipewire::channel::Sender<Command>) -> Self {
        let node_id = node.id;
        let channels = Rc::new(Cell::new(node.volumes.len().max(1)));

        let widget = GtkBox::new(Orientation::Vertical, 6);
        widget.set_width_request(90);
        widget.set_margin_top(4);
        widget.set_margin_bottom(4);

        let name_label = Label::new(Some(node.display_name()));
        name_label.set_wrap(true);
        name_label.set_justify(Justification::Center);
        name_label.set_max_width_chars(12);
        name_label.set_lines(2);
        widget.append(&name_label);

        let scale = Scale::with_range(Orientation::Vertical, 0.0, 1.5, 0.01);
        scale.set_inverted(true);
        scale.set_value(node.volume() as f64);
        scale.set_vexpand(true);
        scale.set_height_request(220);
        scale.set_draw_value(true);
        scale.set_value_pos(PositionType::Bottom);
        {
            let cmd_tx = cmd_tx.clone();
            let channels = channels.clone();
            scale.connect_value_changed(move |s| {
                let v = s.value() as f32;
                let volumes = vec![v; channels.get()];
                let _ = cmd_tx.send(Command::SetVolume { node_id, volumes });
            });
        }
        widget.append(&scale);

        let mute_button = ToggleButton::with_label("Mute");
        mute_button.set_active(node.mute);
        {
            let cmd_tx = cmd_tx.clone();
            mute_button.connect_toggled(move |b| {
                let _ = cmd_tx.send(Command::SetMute { node_id, mute: b.is_active() });
            });
        }
        widget.append(&mute_button);

        // Only hardware devices have a meaningful "default" concept (default.audio.sink/source);
        // app streams and the Antra-style playback/recording streams don't.
        let default_button = if node.is_hardware() {
            let button = Button::with_label("Set Default");
            let node_name = node.name.clone();
            let is_sink = node.media_class == "Audio/Sink";
            {
                let cmd_tx = cmd_tx.clone();
                button.connect_clicked(move |_| {
                    let cmd = if is_sink {
                        Command::SetDefaultSink { node_name: node_name.clone() }
                    } else {
                        Command::SetDefaultSource { node_name: node_name.clone() }
                    };
                    let _ = cmd_tx.send(cmd);
                });
            }
            widget.append(&button);
            Some(button)
        } else {
            None
        };

        Self { widget, group: node.group(), scale, mute_button, default_button, channels }
    }

    /// Refresh the strip's controls to match the node's current state, without re-emitting
    /// `value-changed`/`toggled` signals for values that didn't actually change (GTK only emits
    /// those when the new value differs from the old one, so this is naturally loop-safe).
    pub fn update(&self, node: &NodeInfo, is_default: bool) {
        if !node.volumes.is_empty() {
            self.channels.set(node.volumes.len());
        }
        self.scale.set_value(node.volume() as f64);
        self.mute_button.set_active(node.mute);
        if let Some(button) = &self.default_button {
            button.set_label(if is_default { "Default \u{2713}" } else { "Set Default" });
            button.set_sensitive(!is_default);
        }
    }
}
