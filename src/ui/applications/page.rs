use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{Box as GtkBox, FlowBox, Label, Orientation, ScrolledWindow, SearchEntry, Separator, SelectionMode};

use crate::model::{Graph, MixerGroup, NodeInfo};
use crate::pw::Command;
use crate::ui::overrides::Overrides;
use crate::ui::settings::Settings;
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
    settings: Rc<RefCell<Settings>>,
    playback_box: FlowBox,
    recording_box: FlowBox,
    /// `Rc`-wrapped (unlike most of this struct's other fields, which are only ever accessed via
    /// `&self`) so the search entry's `connect_search_changed` handler, wired up in `new()` before
    /// any `Rc<Self>` exists, can share it directly - see `apply_filter`.
    strips: Rc<RefCell<HashMap<u32, (Strip, Placement)>>>,
    /// Current search/filter text (lowercased), applied by `apply_filter`.
    filter: Rc<RefCell<String>>,
    cmd_tx: pipewire::channel::Sender<Command>,
}

impl ApplicationsPage {
    pub fn new(
        graph: Rc<RefCell<Graph>>,
        cmd_tx: pipewire::channel::Sender<Command>,
        settings: Rc<RefCell<Settings>>,
    ) -> Rc<Self> {
        let widget = GtkBox::new(Orientation::Vertical, 6);
        widget.set_margin_top(12);
        widget.set_margin_bottom(12);
        widget.set_margin_start(12);
        widget.set_margin_end(12);

        let strips: Rc<RefCell<HashMap<u32, (Strip, Placement)>>> = Rc::new(RefCell::new(HashMap::new()));
        let filter = Rc::new(RefCell::new(String::new()));

        let search_entry = SearchEntry::new();
        search_entry.set_placeholder_text(Some("Filter streams..."));
        {
            let strips = strips.clone();
            let filter = filter.clone();
            search_entry.connect_search_changed(move |entry| {
                *filter.borrow_mut() = entry.text().to_lowercase();
                apply_filter(&strips.borrow(), &filter.borrow());
            });
        }
        widget.append(&search_entry);

        let columns = GtkBox::new(Orientation::Horizontal, 12);
        columns.set_vexpand(true);
        widget.append(&columns);

        let (playback_col, playback_box) = make_column("Playback");
        let (recording_col, recording_box) = make_column("Recording");

        columns.append(&playback_col);
        columns.append(&Separator::new(Orientation::Vertical));
        columns.append(&recording_col);

        Rc::new(Self {
            widget,
            graph,
            overrides: Rc::new(RefCell::new(Overrides::load())),
            settings,
            playback_box,
            recording_box,
            strips,
            filter,
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
            let fader_max = self.settings.borrow().fader_max;
            let strip =
                Strip::new(node, display_name, self.cmd_tx.clone(), self.overrides.clone(), resync, fader_max);
            self.flow_box_for(placement).insert(&strip.widget, -1);
            strips.insert(node.id, (strip, placement));
        }

        apply_filter(&strips, &self.filter.borrow());
    }

    /// Fast path for `Event::PeakLevel`, called directly by `ui::app`'s event loop instead of
    /// `sync()` - see `Strip::set_peak` for why.
    pub fn update_peak(&self, node_id: u32, peak: f32) {
        if let Some((strip, _)) = self.strips.borrow().get(&node_id) {
            strip.set_peak(peak);
        }
    }

    /// Apply a new fader ceiling (from the Settings popover, `ui::app`) to every strip already on
    /// screen; newly-created strips pick up `self.settings` directly in `sync()` above.
    pub fn apply_fader_max(&self, fader_max: f32) {
        for (strip, _) in self.strips.borrow().values() {
            strip.set_fader_max(fader_max);
        }
    }
}

/// Show only the strips whose display name matches `filter` (a lowercased substring match, empty
/// meaning "show everything") - called both live as the search entry's text changes and at the
/// end of every `sync()`, so a rename/graph change can't leave a strip's visibility stale.
fn apply_filter(strips: &HashMap<u32, (Strip, Placement)>, filter: &str) {
    for (strip, _) in strips.values() {
        strip.widget.set_visible(filter.is_empty() || strip.display_name().to_lowercase().contains(filter));
    }
}
