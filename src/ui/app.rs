use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{
    glib, Adjustment, Application, ApplicationWindow, Box as GtkBox, Button, HeaderBar, Label, Orientation,
    Popover, SpinButton, Stack, StackSwitcher,
};

use crate::model::Graph;
use crate::pw;
use crate::tray;

use super::applications::ApplicationsPage;
use super::devices::DevicesPage;
use super::patchbay::PatchbayPage;
use super::settings::{self, Settings};

const APP_ID: &str = "ca.millerfamily.SgithiAudioMeter";

pub fn run() -> glib::ExitCode {
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_window);
    app.run()
}

fn build_window(app: &Application) {
    let (cmd_tx, event_rx) = pw::spawn();

    let graph = Rc::new(RefCell::new(Graph::new()));
    let settings = Rc::new(RefCell::new(Settings::load()));
    let devices_page = DevicesPage::new(graph.clone(), cmd_tx.clone(), settings.clone());
    let applications_page = ApplicationsPage::new(graph.clone(), cmd_tx.clone(), settings.clone());
    let patchbay_page = PatchbayPage::new(graph.clone(), cmd_tx.clone());

    let stack = Stack::new();
    stack.add_titled(&devices_page.widget, Some("devices"), "Devices");
    stack.add_titled(&applications_page.widget, Some("applications"), "Applications");
    stack.add_titled(&patchbay_page.widget, Some("patchbay"), "Patchbay");

    let switcher = StackSwitcher::new();
    switcher.set_stack(Some(&stack));

    let header = HeaderBar::new();
    header.set_title_widget(Some(&switcher));

    let settings_button = Button::with_label("\u{2699}"); // gear
    settings_button.add_css_class("flat");
    settings_button.set_tooltip_text(Some("Settings"));
    header.pack_end(&settings_button);
    let settings_popover =
        build_settings_popover(&settings_button, settings, devices_page.clone(), applications_page.clone());
    settings_button.connect_clicked(move |_| settings_popover.popup());

    let window = ApplicationWindow::builder()
        .application(app)
        .title("Sgithi Audio Meter")
        .default_width(1000)
        .default_height(700)
        .child(&stack)
        .build();
    window.set_titlebar(Some(&header));

    // Background mode: the window's own close button hides it rather than quitting (matching a
    // typical tray-icon app), so `really_quit` distinguishes that from an actual quit requested
    // via the tray menu, which calls `window.close()` after setting this first.
    let really_quit = Rc::new(Cell::new(false));
    {
        let cmd_tx = cmd_tx.clone();
        let really_quit = really_quit.clone();
        window.connect_close_request(move |win| {
            if really_quit.get() {
                let _ = cmd_tx.send(pw::Command::Terminate);
                glib::Propagation::Proceed
            } else {
                win.set_visible(false);
                glib::Propagation::Stop
            }
        });
    }

    let tray_rx = tray::spawn();
    {
        let window = window.clone();
        let really_quit = really_quit.clone();
        glib::spawn_future_local(async move {
            while let Ok(event) = tray_rx.recv().await {
                match event {
                    tray::TrayEvent::ToggleWindow => {
                        if window.is_visible() {
                            window.set_visible(false);
                        } else {
                            window.present();
                        }
                    }
                    tray::TrayEvent::Quit => {
                        really_quit.set(true);
                        window.close();
                    }
                }
            }
        });
    }

    let window_for_events = window.clone();
    glib::spawn_future_local(async move {
        while let Ok(event) = event_rx.recv().await {
            // `PeakLevel` arrives far more often (~30Hz per watched node - see `pw::peak`) than
            // every other event, and a full page `sync()` reconciles *all* strips' placement,
            // name, volume and mute state - cheap for an occasional volume change, but expensive
            // enough at peak-meter frequency to peg a CPU core with only a handful of nodes on
            // screen. Route it straight to the one strip it affects instead.
            if let pw::Event::PeakLevel { id, peak } = event {
                graph.borrow_mut().apply(pw::Event::PeakLevel { id, peak });
                devices_page.update_peak(id, peak);
                applications_page.update_peak(id, peak);
                continue;
            }
            // The pipewire thread has exited (see `pw::thread`'s module doc comment for why this
            // doesn't attempt a live reconnect) - say so plainly rather than leaving the mixer
            // looking normal but silently frozen on stale state.
            if let pw::Event::Disconnected = event {
                window_for_events.set_title(Some("Sgithi Audio Meter (disconnected - restart to reconnect)"));
                continue;
            }
            graph.borrow_mut().apply(event);
            devices_page.sync();
            applications_page.sync();
            patchbay_page.sync();
        }
    });

    window.present();
}

/// Build the Settings popover anchored to the header's gear button - currently just the fader
/// ceiling (`ui::settings::Settings::fader_max`), applied live to every strip on both mixer pages
/// as the value changes, and persisted immediately (see `Settings::set_fader_max`).
fn build_settings_popover(
    button: &Button,
    settings: Rc<RefCell<Settings>>,
    devices_page: Rc<DevicesPage>,
    applications_page: Rc<ApplicationsPage>,
) -> Popover {
    let content = GtkBox::new(Orientation::Vertical, 6);
    content.set_margin_top(8);
    content.set_margin_bottom(8);
    content.set_margin_start(8);
    content.set_margin_end(8);
    content.set_width_request(220);

    let fader_row = GtkBox::new(Orientation::Horizontal, 6);
    fader_row.append(&Label::new(Some("Fader max %")));
    let adjustment = Adjustment::new(
        (settings.borrow().fader_max * 100.0) as f64,
        (settings::MIN_FADER_MAX * 100.0) as f64,
        (settings::MAX_FADER_MAX * 100.0) as f64,
        10.0,
        10.0,
        0.0,
    );
    let spin = SpinButton::new(Some(&adjustment), 1.0, 0);
    fader_row.append(&spin);
    content.append(&fader_row);

    let apply_fader_max = {
        let settings = settings.clone();
        let devices_page = devices_page.clone();
        let applications_page = applications_page.clone();
        Rc::new(move |percent: f64| {
            settings.borrow_mut().set_fader_max((percent / 100.0) as f32);
            let fader_max = settings.borrow().fader_max;
            devices_page.apply_fader_max(fader_max);
            applications_page.apply_fader_max(fader_max);
        }) as Rc<dyn Fn(f64)>
    };

    let spin_changed = {
        let apply_fader_max = apply_fader_max.clone();
        spin.connect_value_changed(move |s| apply_fader_max(s.value()))
    };

    let reset_button = Button::with_label("Reset to default");
    {
        let spin = spin.clone();
        reset_button.connect_clicked(move |_| {
            settings.borrow_mut().reset_to_default();
            let fader_max = settings.borrow().fader_max;
            devices_page.apply_fader_max(fader_max);
            applications_page.apply_fader_max(fader_max);
            // Blocked like `Strip::update()`: setting the spin button's value would otherwise
            // re-emit `value-changed` and apply the (already-applied) value a second time.
            spin.block_signal(&spin_changed);
            spin.set_value((fader_max * 100.0) as f64);
            spin.unblock_signal(&spin_changed);
        });
    }
    content.append(&reset_button);

    let popover = Popover::new();
    popover.set_child(Some(&content));
    popover.set_parent(button);
    popover
}
