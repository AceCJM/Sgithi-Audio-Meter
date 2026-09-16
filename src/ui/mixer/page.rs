use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{Box as GtkBox, FlowBox, Label, Orientation, ScrolledWindow, Separator, SelectionMode};

use crate::model::{Graph, MixerGroup};
use crate::pw::Command;

use super::strip::Strip;

/// The Voicemeeter-style mixer page: hardware devices and application streams as vertical
/// fader strips, grouped into an Inputs column and an Outputs column on one page.
pub struct MixerPage {
    pub widget: GtkBox,
    inputs_box: FlowBox,
    outputs_box: FlowBox,
    strips: RefCell<HashMap<u32, Strip>>,
    cmd_tx: pipewire::channel::Sender<Command>,
}

fn make_column(title: &str) -> (GtkBox, FlowBox) {
    let column = GtkBox::new(Orientation::Vertical, 6);
    column.set_hexpand(true);

    let heading = Label::new(Some(title));
    heading.add_css_class("title-4");
    column.append(&heading);

    let flow_box = FlowBox::new();
    flow_box.set_selection_mode(SelectionMode::None);
    flow_box.set_max_children_per_line(64);
    flow_box.set_row_spacing(4);
    flow_box.set_column_spacing(4);

    let scroll = ScrolledWindow::new();
    scroll.set_child(Some(&flow_box));
    scroll.set_vexpand(true);
    scroll.set_hexpand(true);
    column.append(&scroll);

    (column, flow_box)
}

impl MixerPage {
    pub fn new(cmd_tx: pipewire::channel::Sender<Command>) -> Rc<Self> {
        let widget = GtkBox::new(Orientation::Horizontal, 12);
        widget.set_margin_top(12);
        widget.set_margin_bottom(12);
        widget.set_margin_start(12);
        widget.set_margin_end(12);

        let (inputs_col, inputs_box) = make_column("Inputs");
        let (outputs_col, outputs_box) = make_column("Outputs");

        widget.append(&inputs_col);
        widget.append(&Separator::new(Orientation::Vertical));
        widget.append(&outputs_col);

        Rc::new(Self { widget, inputs_box, outputs_box, strips: RefCell::new(HashMap::new()), cmd_tx })
    }

    /// Reconcile strips against the current graph state: create/destroy strips for
    /// added/removed nodes, and refresh volume/mute on the rest.
    pub fn sync(&self, graph: &Graph) {
        let mut strips = self.strips.borrow_mut();

        strips.retain(|id, strip| {
            let keep = graph.nodes.contains_key(id);
            if !keep {
                match strip.group {
                    MixerGroup::Inputs => self.inputs_box.remove(&strip.widget),
                    MixerGroup::Outputs => self.outputs_box.remove(&strip.widget),
                    MixerGroup::Other => {}
                }
            }
            keep
        });

        let mut nodes: Vec<_> = graph.nodes.values().collect();
        nodes.sort_by_key(|n| n.id);
        for node in nodes {
            let is_default = match node.media_class.as_str() {
                "Audio/Sink" => graph.default_sink == Some(node.id),
                "Audio/Source" => graph.default_source == Some(node.id),
                _ => false,
            };
            if let Some(strip) = strips.get(&node.id) {
                strip.update(node, is_default);
                continue;
            }
            let group = node.group();
            if group == MixerGroup::Other {
                continue;
            }
            let strip = Strip::new(node, self.cmd_tx.clone());
            match group {
                MixerGroup::Inputs => self.inputs_box.insert(&strip.widget, -1),
                MixerGroup::Outputs => self.outputs_box.insert(&strip.widget, -1),
                MixerGroup::Other => unreachable!(),
            }
            strips.insert(node.id, strip);
        }
    }
}
