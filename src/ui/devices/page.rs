use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{Box as GtkBox, FlowBox, Label, Orientation, ScrolledWindow, Separator, SelectionMode};

use crate::model::{Graph, MixerGroup, NodeInfo};
use crate::pw::Command;
use crate::ui::overrides::{Category, Overrides};
use crate::ui::strip::Strip;

use super::profile_bar::DevicesBar;

/// Which `FlowBox` a strip currently lives in - tracked per strip (rather than re-derived from
/// `NodeInfo` at removal time) so removal is unambiguous even if the node is already gone from
/// `Graph`, or its category override changed, by the time a strip is torn down/moved.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Placement {
    Microphones,
    OtherInputs,
    Outputs,
}

/// Only hardware devices belong on this page - application streams are on the Applications page
/// instead (see `ui::applications`). A saved category override (see `ui::overrides`) takes
/// precedence over the `is_mic_like()` heuristic.
fn placement_for(node: &NodeInfo, overrides: &Overrides) -> Option<Placement> {
    if !node.is_hardware() {
        return None;
    }
    match node.group() {
        MixerGroup::Inputs => {
            let is_mic = match overrides.category(&node.name) {
                Some(Category::Microphone) => true,
                Some(Category::OtherInput) => false,
                None => node.is_mic_like(),
            };
            Some(if is_mic { Placement::Microphones } else { Placement::OtherInputs })
        }
        MixerGroup::Outputs => Some(Placement::Outputs),
        MixerGroup::Other => None,
    }
}

/// The Devices page: hardware inputs and outputs as vertical fader strips, plus a per-device
/// profile bar. Inputs are split into a Microphones sub-section and an Other Inputs sub-section
/// (line/monitor inputs); Outputs are a single column. Application streams live on the separate
/// Applications page instead (`ui::applications`).
pub struct DevicesPage {
    pub widget: GtkBox,
    graph: Rc<RefCell<Graph>>,
    overrides: Rc<RefCell<Overrides>>,
    devices_bar: Rc<DevicesBar>,
    mic_box: FlowBox,
    other_inputs_box: FlowBox,
    outputs_box: FlowBox,
    strips: RefCell<HashMap<u32, (Strip, Placement)>>,
    cmd_tx: pipewire::channel::Sender<Command>,
}

fn make_section(parent: &GtkBox, title: &str) -> FlowBox {
    let heading = Label::new(Some(title));
    heading.add_css_class("heading");
    heading.set_halign(gtk::Align::Start);
    heading.set_margin_top(4);
    parent.append(&heading);

    let flow_box = FlowBox::new();
    flow_box.set_selection_mode(SelectionMode::None);
    flow_box.set_max_children_per_line(64);
    flow_box.set_row_spacing(4);
    flow_box.set_column_spacing(4);
    parent.append(&flow_box);

    flow_box
}

fn make_column(title: &str) -> (GtkBox, GtkBox) {
    let column = GtkBox::new(Orientation::Vertical, 6);
    column.set_hexpand(true);

    let heading = Label::new(Some(title));
    heading.add_css_class("title-4");
    column.append(&heading);

    let sections = GtkBox::new(Orientation::Vertical, 6);
    let scroll = ScrolledWindow::new();
    scroll.set_child(Some(&sections));
    scroll.set_vexpand(true);
    scroll.set_hexpand(true);
    column.append(&scroll);

    (column, sections)
}

impl DevicesPage {
    pub fn new(graph: Rc<RefCell<Graph>>, cmd_tx: pipewire::channel::Sender<Command>) -> Rc<Self> {
        let widget = GtkBox::new(Orientation::Vertical, 6);
        widget.set_margin_top(12);
        widget.set_margin_bottom(12);
        widget.set_margin_start(12);
        widget.set_margin_end(12);

        let devices_bar = DevicesBar::new(cmd_tx.clone());
        widget.append(&devices_bar.widget);

        let columns = GtkBox::new(Orientation::Horizontal, 12);
        columns.set_vexpand(true);
        widget.append(&columns);

        let (inputs_col, inputs_sections) = make_column("Inputs");
        let mic_box = make_section(&inputs_sections, "Microphones");
        let other_inputs_box = make_section(&inputs_sections, "Other Inputs");

        let (outputs_col, outputs_sections) = make_column("Outputs");
        let outputs_box = make_section(&outputs_sections, "Devices");

        columns.append(&inputs_col);
        columns.append(&Separator::new(Orientation::Vertical));
        columns.append(&outputs_col);

        Rc::new(Self {
            widget,
            graph,
            overrides: Rc::new(RefCell::new(Overrides::load())),
            devices_bar,
            mic_box,
            other_inputs_box,
            outputs_box,
            strips: RefCell::new(HashMap::new()),
            cmd_tx,
        })
    }

    fn flow_box_for(&self, placement: Placement) -> &FlowBox {
        match placement {
            Placement::Microphones => &self.mic_box,
            Placement::OtherInputs => &self.other_inputs_box,
            Placement::Outputs => &self.outputs_box,
        }
    }

    /// Reconcile strips against the current graph state: create/destroy/move strips for
    /// added/removed/re-categorized nodes, and refresh volume/mute/name on the rest. Called after
    /// every graph-changing event, and again (via the `resync` closure passed to each `Strip`)
    /// whenever a rename or category override is saved, since that isn't a PipeWire event.
    pub fn sync(self: &Rc<Self>) {
        let graph = self.graph.borrow();
        self.devices_bar.sync(&graph.devices);

        let overrides = self.overrides.borrow();
        let mut strips = self.strips.borrow_mut();

        strips.retain(|id, (strip, placement)| {
            let keep = graph.nodes.contains_key(id);
            if !keep {
                self.flow_box_for(*placement).remove(&strip.widget);
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
            let display_name = overrides.custom_name(&node.name).unwrap_or_else(|| node.display_name());
            let desired = placement_for(node, &overrides);

            // A category override change can move an already-shown node to a different column;
            // tear down and let it be recreated below rather than trying to migrate it in place.
            let current_placement = strips.get(&node.id).map(|(_, p)| *p);
            if current_placement.is_some() && current_placement != desired {
                if let Some((strip, old_placement)) = strips.remove(&node.id) {
                    self.flow_box_for(old_placement).remove(&strip.widget);
                }
            }

            if let Some((strip, _)) = strips.get(&node.id) {
                strip.update(node, display_name, is_default);
                continue;
            }
            let Some(placement) = desired else { continue };

            let resync = {
                let page = self.clone();
                Rc::new(move || page.sync()) as Rc<dyn Fn()>
            };
            let strip = Strip::new(node, display_name, self.cmd_tx.clone(), self.overrides.clone(), resync);
            self.flow_box_for(placement).insert(&strip.widget, -1);
            strips.insert(node.id, (strip, placement));
        }
    }

    /// Fast path for `Event::PeakLevel`, called directly by `ui::app`'s event loop instead of
    /// `sync()` - see `Strip::set_peak` for why.
    pub fn update_peak(&self, node_id: u32, peak: f32) {
        if let Some((strip, _)) = self.strips.borrow().get(&node_id) {
            strip.set_peak(peak);
        }
    }
}
