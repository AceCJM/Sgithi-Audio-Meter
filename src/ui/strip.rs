use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib::SignalHandlerId;
use gtk::prelude::*;
use gtk::{
    Box as GtkBox, Button, DropDown, Entry, EventControllerFocus, GestureClick, Justification, Label, LevelBar,
    Orientation, Popover, Scale, ToggleButton,
};

use crate::model::{is_sink_like, NodeInfo};
use crate::pw::Command;
use crate::ui::overrides::{Category, Overrides};
use crate::ui::volume;

/// One vertical fader strip for a single node (hardware device or app stream).
pub struct Strip {
    pub widget: GtkBox,
    node_id: u32,
    cmd_tx: pipewire::channel::Sender<Command>,
    name_label: Label,
    scale: Scale,
    scale_changed: SignalHandlerId,
    /// Shows the fader's value as a percentage, and doubles as manual numeric input: typing a
    /// number and pressing Enter (or clicking away) applies it the same way dragging the fader
    /// does - see `commit_volume_entry`.
    volume_entry: Entry,
    mute_button: ToggleButton,
    mute_toggled: SignalHandlerId,
    default_button: Option<Button>,
    peak_meter: LevelBar,
    /// The rename/re-categorize popover built in `build_edit_popover`, parented to the pencil
    /// button - see `Drop` for why `Strip` needs to hold onto it.
    edit_popover: Popover,
    /// Number of channels last reported for this node, so a fader move sets every channel
    /// rather than assuming stereo.
    channels: Rc<Cell<usize>>,
    /// -1.0 (full left) ..= 1.0 (full right); only meaningful (and only shown - see `update()`)
    /// for a 2-channel node. Kept separately from the channel volumes PipeWire reports so the
    /// master fader and the balance control can each be moved independently without the other
    /// clobbering it - see `balance_to_volumes`/`balance_from_volumes`.
    balance: Rc<Cell<f32>>,
    balance_scale: Scale,
    balance_changed: SignalHandlerId,
    /// Per-channel percentages implied by the current fader + balance - see `set_balance_label`.
    balance_label: Label,
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
        fader_max: f32,
    ) -> Self {
        let node_id = node.id;
        let channels = Rc::new(Cell::new(node.volumes.len().max(1)));
        let balance = Rc::new(Cell::new(balance_from_volumes(&node.volumes)));

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

        let edit_popover = build_edit_popover(&edit_button, &name_label, node, overrides, resync);

        // The fader works in perceptual (cubic) units, not the linear amplitude PipeWire sends
        // over the wire - see `ui::volume`. `fader_max` defaults to 1.5 (pavucontrol's 150%
        // ceiling) but is user-configurable via the Settings popover - see `ui::settings`.
        let scale = Scale::with_range(Orientation::Vertical, 0.0, fader_max as f64, 0.01);
        scale.set_inverted(true);
        scale.set_value(volume::linear_to_perceptual(node.volume()) as f64);
        scale.set_vexpand(true);
        scale.set_height_request(220);
        // The built-in numeric readout shows the raw 0.0-1.5 float; we show our own percentage
        // label instead so we can also flag over-amplification.
        scale.set_draw_value(false);

        // Double-click resets to unity gain (100%) - matches the fader's own perceptual units, so
        // this is just `1.0` regardless of `fader_max`. Capture phase + claiming the sequence on
        // the second press stops the Scale's own click-to-jump handling from processing that same
        // click afterward and overriding the reset with wherever it was clicked.
        let volume_reset = GestureClick::new();
        volume_reset.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let scale = scale.clone();
            volume_reset.connect_pressed(move |gesture, n_press, _x, _y| {
                if n_press == 2 {
                    scale.set_value(1.0);
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                }
            });
        }
        scale.add_controller(volume_reset);

        let volume_entry = Entry::new();
        volume_entry.set_width_chars(5);
        volume_entry.set_max_width_chars(5);
        gtk::prelude::EditableExt::set_alignment(&volume_entry, 0.5);
        volume_entry.set_tooltip_text(Some("Click to type an exact percentage"));
        set_volume_entry(&volume_entry, node.volume());

        let balance_label = Label::new(None);

        let scale_changed = {
            let cmd_tx = cmd_tx.clone();
            let channels = channels.clone();
            let volume_entry = volume_entry.clone();
            let balance = balance.clone();
            let balance_label = balance_label.clone();
            scale.connect_value_changed(move |s| {
                let perceptual = s.value() as f32;
                let linear = volume::perceptual_to_linear(perceptual);
                set_volume_entry(&volume_entry, linear);
                set_balance_label(&balance_label, linear, balance.get());
                let volumes = balance_to_volumes(linear, balance.get(), channels.get());
                let _ = cmd_tx.send(Command::SetVolume { node_id, volumes });
            })
        };

        // Typing a number and pressing Enter, or clicking away, applies it exactly like dragging
        // the fader to that position would (routed through the same `scale.set_value()` call, so
        // it gets the same balance-aware command and the same display update - see `scale_changed`
        // above). An unparseable value just redisplays the fader's current one, discarding the typo.
        let commit_volume_entry = {
            let scale = scale.clone();
            Rc::new(move |entry: &Entry| {
                let adjustment = scale.adjustment();
                match parse_percent(&entry.text(), adjustment.lower() as f32, adjustment.upper() as f32) {
                    Some(perceptual) => scale.set_value(perceptual as f64),
                    None => set_volume_entry(entry, volume::perceptual_to_linear(scale.value() as f32)),
                }
            }) as Rc<dyn Fn(&Entry)>
        };
        {
            let commit_volume_entry = commit_volume_entry.clone();
            volume_entry.connect_activate(move |entry| commit_volume_entry(entry));
        }
        {
            let commit_volume_entry = commit_volume_entry.clone();
            let entry_for_focus = volume_entry.clone();
            let focus = EventControllerFocus::new();
            focus.connect_leave(move |_| commit_volume_entry(&entry_for_focus));
            volume_entry.add_controller(focus);
        }

        // Balance is only meaningful for a 2-channel node (see `balance_to_volumes`) - hidden
        // otherwise. Re-checked in `update()` too, since a node's `Strip` is often created before
        // its first `Props`/Route volume param (and so its real channel count) has arrived.
        let balance_scale = Scale::with_range(Orientation::Horizontal, -1.0, 1.0, 0.05);
        balance_scale.set_value(balance.get() as f64);
        balance_scale.set_draw_value(false);
        balance_scale.set_width_request(80);
        balance_scale.set_tooltip_text(Some("Balance - double-click to center"));
        balance_scale.set_visible(node.volumes.len() == 2);
        balance_label.set_visible(node.volumes.len() == 2);
        set_balance_label(&balance_label, node.volume(), balance.get());

        let balance_reset = GestureClick::new();
        balance_reset.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let balance_scale = balance_scale.clone();
            balance_reset.connect_pressed(move |gesture, n_press, _x, _y| {
                if n_press == 2 {
                    balance_scale.set_value(0.0);
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                }
            });
        }
        balance_scale.add_controller(balance_reset);

        let balance_changed = {
            let cmd_tx = cmd_tx.clone();
            let channels = channels.clone();
            let balance = balance.clone();
            let scale = scale.clone();
            let balance_label = balance_label.clone();
            balance_scale.connect_value_changed(move |b| {
                let new_balance = b.value() as f32;
                balance.set(new_balance);
                let linear = volume::perceptual_to_linear(scale.value() as f32);
                set_balance_label(&balance_label, linear, new_balance);
                let volumes = balance_to_volumes(linear, new_balance, channels.get());
                let _ = cmd_tx.send(Command::SetVolume { node_id, volumes });
            })
        };

        // Read-only live level meter (`pw::peak`), fed by `Event::PeakLevel` via `update()` below
        // - not a control, so unlike the fader/mute button it needs no signal-blocking treatment.
        let peak_meter = LevelBar::new();
        peak_meter.set_orientation(Orientation::Vertical);
        peak_meter.set_inverted(true);
        peak_meter.set_min_value(0.0);
        peak_meter.set_max_value(1.0);
        peak_meter.set_height_request(220);
        peak_meter.set_width_request(10);

        let fader_row = GtkBox::new(Orientation::Horizontal, 4);
        fader_row.set_vexpand(true);
        fader_row.append(&scale);
        fader_row.append(&peak_meter);
        widget.append(&fader_row);
        widget.append(&volume_entry);
        widget.append(&balance_scale);
        widget.append(&balance_label);

        let _ = cmd_tx.send(Command::WatchPeak {
            node_id,
            node_name: node.name.clone(),
            capture_sink: is_sink_like(&node.media_class),
        });

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
            node_id,
            cmd_tx,
            name_label,
            scale,
            scale_changed,
            volume_entry,
            mute_button,
            mute_toggled,
            default_button,
            peak_meter,
            edit_popover,
            channels,
            balance,
            balance_scale,
            balance_changed,
            balance_label,
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
        // Don't clobber the entry while the user is actively typing a replacement value into it.
        if !self.volume_entry.has_focus() {
            set_volume_entry(&self.volume_entry, node.volume());
        }

        let is_stereo = node.volumes.len() == 2;
        self.balance_scale.set_visible(is_stereo);
        self.balance_label.set_visible(is_stereo);
        if is_stereo {
            let balance = balance_from_volumes(&node.volumes);
            self.balance.set(balance);
            self.balance_scale.block_signal(&self.balance_changed);
            self.balance_scale.set_value(balance as f64);
            self.balance_scale.unblock_signal(&self.balance_changed);
            set_balance_label(&self.balance_label, node.volume(), balance);
        }

        self.mute_button.block_signal(&self.mute_toggled);
        self.mute_button.set_active(node.mute);
        self.mute_button.unblock_signal(&self.mute_toggled);

        if let Some(button) = &self.default_button {
            button.set_label(if is_default { "Default \u{2713}" } else { "Set Default" });
            button.set_sensitive(!is_default);
        }

        self.peak_meter.set_value(node.peak.min(1.0) as f64);
    }

    /// The name currently shown on the strip - for the search/filter box (`ui::devices::page`,
    /// `ui::applications::page`) to match against, rather than needing its own separate copy of
    /// what's effectively already display state.
    pub fn display_name(&self) -> String {
        self.name_label.text().to_string()
    }

    /// Apply a new fader ceiling from the Settings popover (`ui::settings::Settings::fader_max`).
    /// Signal-blocked like `update()`: lowering the ceiling below the fader's current displayed
    /// value clamps it, which - unblocked - would re-emit `value-changed` and send a real
    /// `SetVolume` command for a change the user never made.
    pub fn set_fader_max(&self, fader_max: f32) {
        let current = self.scale.value();
        self.scale.block_signal(&self.scale_changed);
        self.scale.set_range(0.0, fader_max as f64);
        self.scale.set_value(current);
        self.scale.unblock_signal(&self.scale_changed);
    }

    /// Update just the peak meter, bypassing everything else `update()` touches. `Event::PeakLevel`
    /// arrives at up to ~30Hz *per node* (see `pw::peak`) - routing it through a full page `sync()`
    /// (which reconciles every strip's placement, name, volume and mute state) made a handful of
    /// watched nodes alone peg a CPU core, so `ui::app`'s event loop calls this directly instead.
    pub fn set_peak(&self, peak: f32) {
        self.peak_meter.set_value(peak.min(1.0) as f64);
    }
}

impl Drop for Strip {
    fn drop(&mut self) {
        // Stop this node's metering stream (started in `Strip::new` via `Command::WatchPeak`)
        // once its strip is no longer shown - `PwState.peaks` in the PipeWire thread otherwise has
        // no other way to learn a node's strip went away (a category change or node removal just
        // drops the `Strip` value, it isn't a PipeWire event).
        let _ = self.cmd_tx.send(Command::UnwatchPeak { node_id: self.node_id });

        // `edit_popover` is `set_parent()`ed to the pencil button (see `build_edit_popover`)
        // rather than owned by a container `add`/`append` call, so removing `widget` from its
        // `FlowBox` never detaches it - without this, dropping the strip finalizes the pencil
        // button while the popover is still attached, which GTK logs as "Finalizing GtkButton...
        // but it still has children left" (caught live: switching a device's profile, which tears
        // down and recreates all its strips, produced a wave of these).
        self.edit_popover.unparent();
    }
}

/// Build the rename/re-categorize popover anchored to `edit_button`, and wire it up.
fn build_edit_popover(
    edit_button: &Button,
    name_label: &Label,
    node: &NodeInfo,
    overrides: Rc<RefCell<Overrides>>,
    resync: Rc<dyn Fn()>,
) -> Popover {
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

    popover
}

/// Show a linear volume as a percentage in the entry, styled red (GTK's built-in "error" semantic
/// class) when over 100% - mirroring pavucontrol's over-amplification warning.
fn set_volume_entry(entry: &Entry, linear: f32) {
    let perceptual = volume::linear_to_perceptual(linear);
    entry.set_text(&format!("{:.0}%", perceptual * 100.0));
    if perceptual > 1.0 {
        entry.add_css_class("error");
    } else {
        entry.remove_css_class("error");
    }
}

/// Parse manually-typed fader input (e.g. "120", "120%", "  87 % ") into a clamped perceptual
/// value, or `None` if it isn't a number - the caller then leaves the fader wherever it already
/// was rather than applying anything.
fn parse_percent(text: &str, min: f32, max: f32) -> Option<f32> {
    let trimmed = text.trim().trim_end_matches('%').trim();
    let percent: f32 = trimmed.parse().ok()?;
    Some((percent / 100.0).clamp(min, max))
}

/// Show the per-channel percentages a stereo balance implies (see `balance_to_volumes`) - e.g.
/// "L 100% · R 60%" for a master fader at 100% panned slightly right.
fn set_balance_label(label: &Label, master_linear: f32, balance: f32) {
    let volumes = balance_to_volumes(master_linear, balance, 2);
    let left = volume::linear_to_perceptual(volumes[0]) * 100.0;
    let right = volume::linear_to_perceptual(volumes[1]) * 100.0;
    label.set_text(&format!("L {left:.0}% \u{b7} R {right:.0}%"));
}

/// Apply a stereo balance (-1.0 full left ..= 0.0 center ..= 1.0 full right, pavucontrol's
/// convention: the louder channel stays at `master`, the other is attenuated) to a master linear
/// volume. Channel counts other than 2 ignore balance entirely, matching pavucontrol, which also
/// only exposes balance for stereo devices.
fn balance_to_volumes(master: f32, balance: f32, channel_count: usize) -> Vec<f32> {
    if channel_count != 2 {
        return vec![master; channel_count];
    }
    if balance <= 0.0 {
        vec![master, master * (1.0 + balance)]
    } else {
        vec![master * (1.0 - balance), master]
    }
}

/// The inverse of `balance_to_volumes`: recover the balance implied by a pair of channel volumes,
/// e.g. after an external tool (or this app's own round-tripped command) changes them. `0.0` for
/// anything other than exactly 2 channels, or a silent (both-zero) pair.
fn balance_from_volumes(volumes: &[f32]) -> f32 {
    let &[left, right] = volumes else { return 0.0 };
    if left >= right {
        if left > 0.0 {
            right / left - 1.0
        } else {
            0.0
        }
    } else if right > 0.0 {
        1.0 - left / right
    } else {
        0.0
    }
}

#[cfg(test)]
mod balance_tests {
    use super::*;

    #[test]
    fn parse_percent_accepts_plain_and_percent_suffixed_numbers() {
        assert_eq!(parse_percent("100", 0.0, 1.5), Some(1.0));
        assert_eq!(parse_percent("100%", 0.0, 1.5), Some(1.0));
        assert_eq!(parse_percent("  74 % ", 0.0, 1.5), Some(0.74));
    }

    #[test]
    fn parse_percent_clamps_to_the_faders_range() {
        assert_eq!(parse_percent("500", 0.0, 1.5), Some(1.5));
        assert_eq!(parse_percent("-20", 0.0, 1.5), Some(0.0));
    }

    #[test]
    fn parse_percent_rejects_non_numbers() {
        assert_eq!(parse_percent("abc", 0.0, 1.5), None);
        assert_eq!(parse_percent("", 0.0, 1.5), None);
    }

    #[test]
    fn centered_balance_is_equal_channels() {
        assert_eq!(balance_to_volumes(0.8, 0.0, 2), vec![0.8, 0.8]);
    }

    #[test]
    fn negative_balance_attenuates_right_channel() {
        assert_eq!(balance_to_volumes(0.8, -0.5, 2), vec![0.8, 0.4]);
    }

    #[test]
    fn positive_balance_attenuates_left_channel() {
        assert_eq!(balance_to_volumes(0.8, 0.5, 2), vec![0.4, 0.8]);
    }

    #[test]
    fn non_stereo_ignores_balance() {
        assert_eq!(balance_to_volumes(0.8, 0.5, 1), vec![0.8]);
        assert_eq!(balance_to_volumes(0.8, -1.0, 3), vec![0.8, 0.8, 0.8]);
    }

    #[test]
    fn balance_round_trips_through_volumes() {
        for balance in [-1.0_f32, -0.5, -0.25, 0.0, 0.25, 0.5, 1.0] {
            let volumes = balance_to_volumes(0.8, balance, 2);
            let recovered = balance_from_volumes(&volumes);
            assert!((recovered - balance).abs() < 1e-5, "balance={balance} recovered={recovered}");
        }
    }

    #[test]
    fn balance_from_volumes_ignores_non_stereo_and_silence() {
        assert_eq!(balance_from_volumes(&[0.5]), 0.0);
        assert_eq!(balance_from_volumes(&[0.5, 0.5, 0.5]), 0.0);
        assert_eq!(balance_from_volumes(&[0.0, 0.0]), 0.0);
    }
}
