use std::cell::Cell;
use std::rc::Rc;

use gtk::glib::SignalHandlerId;
use gtk::prelude::*;
use gtk::{Box as GtkBox, Button, Justification, Label, Orientation, Scale, ToggleButton};

use crate::model::{MixerGroup, NodeInfo};
use crate::pw::Command;
use crate::ui::volume;

/// One vertical fader strip for a single node (hardware device or app stream).
pub struct Strip {
    pub widget: GtkBox,
    pub group: MixerGroup,
    scale: Scale,
    scale_changed: SignalHandlerId,
    volume_label: Label,
    mute_button: ToggleButton,
    mute_toggled: SignalHandlerId,
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

        // The fader works in perceptual (cubic) units, not the linear amplitude PipeWire sends
        // over the wire - see `ui::volume`. Range 0.0-1.5 matches pavucontrol's 0%-150%.
        let scale = Scale::with_range(Orientation::Vertical, 0.0, 1.5, 0.01);
        scale.set_inverted(true);
        scale.set_value(volume::linear_to_perceptual(node.volume()) as f64);
        scale.set_vexpand(true);
        scale.set_height_request(220);
        // The built-in numeric readout shows the raw 0.0-1.5 float; we show our own percentage
        // label instead so we can also flag over-amplification.
        scale.set_draw_value(false);

        let volume_label = Label::new(None);
        set_volume_label(&volume_label, node.volume());

        let scale_changed = {
            let cmd_tx = cmd_tx.clone();
            let channels = channels.clone();
            let volume_label = volume_label.clone();
            scale.connect_value_changed(move |s| {
                let perceptual = s.value() as f32;
                let linear = volume::perceptual_to_linear(perceptual);
                set_volume_label(&volume_label, linear);
                let volumes = vec![linear; channels.get()];
                let _ = cmd_tx.send(Command::SetVolume { node_id, volumes });
            })
        };
        widget.append(&scale);
        widget.append(&volume_label);

        let mute_button = ToggleButton::with_label("Mute");
        mute_button.set_active(node.mute);
        let mute_toggled = {
            let cmd_tx = cmd_tx.clone();
            mute_button.connect_toggled(move |b| {
                let _ = cmd_tx.send(Command::SetMute { node_id, mute: b.is_active() });
            })
        };
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

        Self {
            widget,
            group: node.group(),
            scale,
            scale_changed,
            volume_label,
            mute_button,
            mute_toggled,
            default_button,
            channels,
        }
    }

    /// Refresh the strip's controls to match the node's current state. Signal handlers are
    /// blocked around the programmatic `set_value`/`set_active` calls below: without that, an
    /// incoming state update whose value differs from the widget's current one re-emits
    /// `value-changed`/`toggled` (GTK only suppresses emission when the value is unchanged, which
    /// does NOT hold here - `sync()` calls this for every node on every graph event), which would
    /// otherwise send a command right back to PipeWire for a change the user never made. Only a
    /// genuine user interaction with the widget should ever produce an outgoing command.
    pub fn update(&self, node: &NodeInfo, is_default: bool) {
        if !node.volumes.is_empty() {
            self.channels.set(node.volumes.len());
        }

        self.scale.block_signal(&self.scale_changed);
        self.scale.set_value(volume::linear_to_perceptual(node.volume()) as f64);
        self.scale.unblock_signal(&self.scale_changed);
        set_volume_label(&self.volume_label, node.volume());

        self.mute_button.block_signal(&self.mute_toggled);
        self.mute_button.set_active(node.mute);
        self.mute_button.unblock_signal(&self.mute_toggled);

        if let Some(button) = &self.default_button {
            button.set_label(if is_default { "Default \u{2713}" } else { "Set Default" });
            button.set_sensitive(!is_default);
        }
    }
}

/// Show a linear volume as a percentage, styled red (GTK's built-in "error" semantic class) when
/// over 100% - mirroring pavucontrol's over-amplification warning.
fn set_volume_label(label: &Label, linear: f32) {
    let perceptual = volume::linear_to_perceptual(linear);
    label.set_text(&format!("{:.0}%", perceptual * 100.0));
    if perceptual > 1.0 {
        label.add_css_class("error");
    } else {
        label.remove_css_class("error");
    }
}
