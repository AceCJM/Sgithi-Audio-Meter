use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{glib, Application, ApplicationWindow, HeaderBar, Stack, StackSwitcher};

use crate::model::Graph;
use crate::pw;

use super::applications::ApplicationsPage;
use super::devices::DevicesPage;
use super::patchbay::PatchbayPage;

const APP_ID: &str = "ca.millerfamily.SgithiAudioMeter";

pub fn run() -> glib::ExitCode {
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_window);
    app.run()
}

fn build_window(app: &Application) {
    let (cmd_tx, event_rx) = pw::spawn();

    let graph = Rc::new(RefCell::new(Graph::new()));
    let devices_page = DevicesPage::new(graph.clone(), cmd_tx.clone());
    let applications_page = ApplicationsPage::new(graph.clone(), cmd_tx.clone());
    let patchbay_page = PatchbayPage::new(graph.clone(), cmd_tx.clone());

    let stack = Stack::new();
    stack.add_titled(&devices_page.widget, Some("devices"), "Devices");
    stack.add_titled(&applications_page.widget, Some("applications"), "Applications");
    stack.add_titled(&patchbay_page.widget, Some("patchbay"), "Patchbay");

    let switcher = StackSwitcher::new();
    switcher.set_stack(Some(&stack));

    let header = HeaderBar::new();
    header.set_title_widget(Some(&switcher));

    let window = ApplicationWindow::builder()
        .application(app)
        .title("Sgithi Audio Meter")
        .default_width(1000)
        .default_height(700)
        .child(&stack)
        .build();
    window.set_titlebar(Some(&header));

    {
        let cmd_tx = cmd_tx.clone();
        window.connect_close_request(move |_| {
            let _ = cmd_tx.send(pw::Command::Terminate);
            glib::Propagation::Proceed
        });
    }

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
            graph.borrow_mut().apply(event);
            devices_page.sync();
            applications_page.sync();
            patchbay_page.sync();
        }
    });

    window.present();
}
