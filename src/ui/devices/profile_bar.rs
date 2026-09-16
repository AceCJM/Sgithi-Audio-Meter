use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::glib::SignalHandlerId;
use gtk::prelude::*;
use gtk::{Box as GtkBox, DropDown, Label, Orientation, StringList};

use crate::model::DeviceInfo;
use crate::pw::Command;

/// One "Device Name: [Profile v]" row. Only shown for devices with more than one profile to
/// choose between - a device with a single fixed profile has nothing to select.
struct DeviceRow {
    widget: GtkBox,
    dropdown: DropDown,
    dropdown_changed: SignalHandlerId,
    /// Maps a dropdown list position to the PipeWire profile index it represents (not
    /// necessarily the same numbers, and not necessarily contiguous).
    indices: Rc<RefCell<Vec<i32>>>,
}

impl DeviceRow {
    fn new(device: &DeviceInfo, cmd_tx: pipewire::channel::Sender<Command>) -> Self {
        let device_id = device.id;

        let widget = GtkBox::new(Orientation::Horizontal, 6);
        let label = Label::new(Some(&device.name));
        widget.append(&label);

        let dropdown = DropDown::from_strings(&[]);
        let indices: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));

        let dropdown_changed = {
            let indices = indices.clone();
            dropdown.connect_selected_notify(move |d| {
                let position = d.selected();
                if let Some(&profile_index) = indices.borrow().get(position as usize) {
                    let _ = cmd_tx.send(Command::SetProfile { device_id, profile_index });
                }
            })
        };
        widget.append(&dropdown);

        let row = Self { widget, dropdown, dropdown_changed, indices };
        row.update(device);
        row
    }

    fn update(&self, device: &DeviceInfo) {
        let names: Vec<&str> = device.profiles.iter().map(|p| p.description.as_str()).collect();
        let selected_position =
            device.active_profile.and_then(|active| device.profiles.iter().position(|p| p.index == active));

        self.dropdown.block_signal(&self.dropdown_changed);
        self.dropdown.set_model(Some(&StringList::new(&names)));
        self.dropdown.set_selected(selected_position.map(|p| p as u32).unwrap_or(gtk::INVALID_LIST_POSITION));
        self.dropdown.unblock_signal(&self.dropdown_changed);

        *self.indices.borrow_mut() = device.profiles.iter().map(|p| p.index).collect();
    }
}

/// A horizontal bar of per-device profile selectors, shown above the mixer columns. Empty (and
/// so invisible - a `GtkBox` with no children takes no space) until a device with more than one
/// selectable profile is discovered.
pub struct DevicesBar {
    pub widget: GtkBox,
    rows: RefCell<HashMap<u32, DeviceRow>>,
    cmd_tx: pipewire::channel::Sender<Command>,
}

impl DevicesBar {
    pub fn new(cmd_tx: pipewire::channel::Sender<Command>) -> Rc<Self> {
        let widget = GtkBox::new(Orientation::Horizontal, 16);
        widget.set_margin_bottom(8);
        Rc::new(Self { widget, rows: RefCell::new(HashMap::new()), cmd_tx })
    }

    pub fn sync(&self, devices: &HashMap<u32, DeviceInfo>) {
        let mut rows = self.rows.borrow_mut();

        rows.retain(|id, row| {
            let keep = devices.get(id).is_some_and(shows_row);
            if !keep {
                self.widget.remove(&row.widget);
            }
            keep
        });

        let mut devices: Vec<_> = devices.values().collect();
        devices.sort_by_key(|d| d.id);
        for device in devices {
            if !shows_row(device) {
                continue;
            }
            if let Some(row) = rows.get(&device.id) {
                row.update(device);
                continue;
            }
            let row = DeviceRow::new(device, self.cmd_tx.clone());
            self.widget.append(&row.widget);
            rows.insert(device.id, row);
        }
    }
}

/// A device only gets a row once it has more than one profile to choose between *and* we
/// actually know which one is active - creating the dropdown before the active profile is known
/// would show a wrong/default-looking selection (typically the first, alphabetically-sorted
/// profile) until a later update corrects it, which is exactly the "shows the default option,
/// not the selected one" symptom this avoids by simply not showing anything prematurely.
fn shows_row(device: &DeviceInfo) -> bool {
    device.profiles.len() > 1 && device.active_profile.is_some()
}
