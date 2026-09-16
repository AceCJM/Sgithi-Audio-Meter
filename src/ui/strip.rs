use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib::SignalHandlerId;
use gtk::prelude::*;
use gtk::{Box as GtkBox, Button, DropDown, Entry, Justification, Label, Orientation, Popover, Scale, ToggleButton};

use crate::model::NodeInfo;
use crate::pw::Command;
use crate::ui::overrides::{Category, Overrides};
use crate::ui::volume;

/// One vertical fader strip for a single node (hardware device or app stream).
pub struct Strip {
    pub widget: GtkBox,
    name_label: Label,
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
    /// `display_name` is pre-resolved by the caller (falls back from a saved override to
    /// `node.display_name()`) so `Strip` doesn't need to know about `Overrides` lookup itself -
    /// it only needs it for the rename/re-categorize popover's own read-modify-save cycle.
    ///
    /// `resync`, called after a rename or category change is saved, should re-run the owning
    /// page's placement logic - a category change can move this node into a different column.
    pub fn new(
        node: &NodeInfo,
        display_name: &str,
        cmd_tx: pipewire::channel::Sender<Command>,
        overrides: Rc<RefCell<Overrides>>,
        resync: Rc<dyn Fn()>,
    ) -> Self {
        let node_id = node.id;
        let channels = Rc::new(Cell::new(node.volumes.len().max(1)));

        let widget = GtkBox::new(Orientation::Vertical, 6);
        widget.set_width_request(90);
        widget.set_margin_top(4);
        widget.set_margin_bottom(4);

        let name_row = GtkBox::new(Orientation::Horizontal, 2);
        name_row.set_halign(gtk::Align::Center);
        let name_label = Label::new(Some(display_name));
        name_label.set_wrap(true);
        name_label.set_justify(Justification::Center);
        name_label.set_max_width_chars(12);
        name_label.set_lines(2);
        name_row.append(&name_label);

        let edit_button = Button::with_label("\u{270E}"); // pencil
        edit_button.add_css_class("flat");
        edit_button.set_valign(gtk::Align::Start);
        edit_button.set_tooltip_text(Some("Rename / categorize"));
        name_row.append(&edit_button);
        widget.append(&name_row);

        build_edit_popover(&edit_button, &name_label, node, overrides, resync);

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
            name_label,
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
    pub fn update(&self, node: &NodeInfo, display_name: &str, is_default: bool) {
        if self.name_label.text() != display_name {
            self.name_label.set_text(display_name);
        }

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

/// Build the rename/re-categorize popover anchored to `edit_button`, and wire it up.
fn build_edit_popover(
    edit_button: &Button,
    name_label: &Label,
    node: &NodeInfo,
    overrides: Rc<RefCell<Overrides>>,
    resync: Rc<dyn Fn()>,
) {
    let node_name = node.name.clone();
    let pipewire_name = node.display_name().to_string();
    // Only hardware capture devices have a Microphone/Other Input categorization to override -
    // see `NodeInfo::is_mic_like()`.
    let category_eligible = node.is_hardware() && node.media_class == "Audio/Source";

    let content = GtkBox::new(Orientation::Vertical, 6);
    content.set_margin_top(8);
    content.set_margin_bottom(8);
    content.set_margin_start(8);
    content.set_margin_end(8);
    content.set_width_request(180);

    let entry = Entry::new();
    {
        let overrides = overrides.borrow();
        entry.set_text(overrides.custom_name(&node_name).unwrap_or(&pipewire_name));
    }
    content.append(&entry);

    let category_dropdown = if category_eligible {
        let dropdown = DropDown::from_strings(&["Microphone", "Other Input"]);
        let current = overrides.borrow().category(&node_name).unwrap_or(if node.is_mic_like() {
            Category::Microphone
        } else {
            Category::OtherInput
        });
        dropdown.set_selected(if current == Category::Microphone { 0 } else { 1 });
        content.append(&dropdown);
        Some(dropdown)
    } else {
        None
    };

    let button_row = GtkBox::new(Orientation::Horizontal, 6);
    let save_button = Button::with_label("Save");
    let reset_button = Button::with_label("Reset to default");
    button_row.append(&save_button);
    button_row.append(&reset_button);
    content.append(&button_row);

    let popover = Popover::new();
    popover.set_child(Some(&content));
    popover.set_parent(edit_button);

    {
        let popover = popover.clone();
        edit_button.connect_clicked(move |_| popover.popup());
    }

    let do_save = {
        let popover = popover.clone();
        let name_label = name_label.clone();
        let entry = entry.clone();
        let node_name = node_name.clone();
        let pipewire_name = pipewire_name.clone();
        let overrides = overrides.clone();
        let resync = resync.clone();
        move || {
            let typed = entry.text().to_string();
            let typed = typed.trim();
            let name = if typed.is_empty() || typed == pipewire_name { None } else { Some(typed.to_string()) };
            let category = category_dropdown.as_ref().map(|d| {
                if d.selected() == 0 {
                    Category::Microphone
                } else {
                    Category::OtherInput
                }
            });
            overrides.borrow_mut().set(&node_name, name.clone(), category);
            name_label.set_text(name.as_deref().unwrap_or(&pipewire_name));
            popover.popdown();
            resync();
        }
    };
    {
        let do_save = do_save.clone();
        save_button.connect_clicked(move |_| do_save());
    }
    entry.connect_activate(move |_| do_save());

    {
        let popover = popover.clone();
        let name_label = name_label.clone();
        let entry = entry.clone();
        let pipewire_name = pipewire_name.clone();
        reset_button.connect_clicked(move |_| {
            overrides.borrow_mut().set(&node_name, None, None);
            name_label.set_text(&pipewire_name);
            entry.set_text(&pipewire_name);
            popover.popdown();
            resync();
        });
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
