use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{Box as GtkBox, FlowBox, Label, Orientation, ScrolledWindow, Separator, SelectionMode};

use crate::model::{Graph, MixerGroup, NodeInfo};
use crate::pw::Command;
use crate::ui::overrides::Overrides;
use crate::ui::strip::Strip;

/// Which column a strip currently lives in - tracked per strip (rather than re-derived from
/// `NodeInfo` at removal time) so removal is unambiguous even if the node is already gone from
/// `Graph` by the time a strip is torn down.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Placement {
    Playback,
    Recording,
}

/// Only application streams belong on this page - hardware devices are on the separate Devices
/// page instead (`ui::devices`).
fn placement_for(node: &NodeInfo) -> Option<Placement> {
    if node.is_hardware() {
        return None;
    }
    match node.group() {
        MixerGroup::Inputs => Some(Placement::Playback),
        MixerGroup::Outputs => Some(Placement::Recording),
        MixerGroup::Other => None,
    }
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

/// The Applications page: every non-hardware stream (app playback and recording) as vertical
/// fader strips, in a Playback column and a Recording column. Pairs with the Devices page, which
/// covers everything hardware instead.
pub struct ApplicationsPage {
    pub widget: GtkBox,
    graph: Rc<RefCell<Graph>>,
    overrides: Rc<RefCell<Overrides>>,
    playback_box: FlowBox,
    recording_box: FlowBox,
    strips: RefCell<HashMap<u32, (Strip, Placement)>>,
    cmd_tx: pipewire::channel::Sender<Command>,
}

impl ApplicationsPage {
    pub fn new(graph: Rc<RefCell<Graph>>, cmd_tx: pipewire::channel::Sender<Command>) -> Rc<Self> {
        let widget = GtkBox::new(Orientation::Horizontal, 12);
        widget.set_margin_top(12);
        widget.set_margin_bottom(12);
        widget.set_margin_start(12);
        widget.set_margin_end(12);

        let (playback_col, playback_box) = make_column("Playback");
        let (recording_col, recording_box) = make_column("Recording");

        widget.append(&playback_col);
        widget.append(&Separator::new(Orientation::Vertical));
        widget.append(&recording_col);

        Rc::new(Self {
            widget,
            graph,
            overrides: Rc::new(RefCell::new(Overrides::load())),
            playback_box,
            recording_box,
            strips: RefCell::new(HashMap::new()),
            cmd_tx,
        })
    }

    fn flow_box_for(&self, placement: Placement) -> &FlowBox {
        match placement {
            Placement::Playback => &self.playback_box,
            Placement::Recording => &self.recording_box,
        }
    }

    /// Reconcile strips against the current graph state: create/destroy strips for
    /// added/removed streams, and refresh volume/mute/name on the rest. A stream's Playback vs
    /// Recording placement is fixed by its media class (unlike Devices' mic/other split, there's
    /// no override for it), so unlike `DevicesPage::sync` there's no need to ever move a strip.
    pub fn sync(self: &Rc<Self>) {
        let graph = self.graph.borrow();
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
            let display_name = overrides.custom_name(&node.name).unwrap_or_else(|| node.display_name());
            if let Some((strip, _)) = strips.get(&node.id) {
                // App streams have no "default" concept, unlike hardware devices.
                strip.update(node, display_name, false);
                continue;
            }
            let Some(placement) = placement_for(node) else { continue };

            let resync = {
                let page = self.clone();
                Rc::new(move || page.sync()) as Rc<dyn Fn()>
            };
            let strip = Strip::new(node, display_name, self.cmd_tx.clone(), self.overrides.clone(), resync);
            self.flow_box_for(placement).insert(&strip.widget, -1);
            strips.insert(node.id, (strip, placement));
        }
    }
}
