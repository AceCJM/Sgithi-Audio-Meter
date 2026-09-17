//! System tray icon (StatusNotifierItem, via `ksni`) enabling background mode - see `ui::app` for
//! how closing the main window hides it instead of quitting.
//!
//! `ksni::blocking::TrayMethods::spawn` runs its own D-Bus thread, matching `pw::thread`'s
//! pattern: tray/menu callbacks fire on that thread, never touching GTK directly, and are turned
//! into `TrayEvent`s sent to the GTK thread over an `async_channel` - the same as `pw::Event`.

use ksni::blocking::TrayMethods;
use ksni::menu::{MenuItem, StandardItem};

#[derive(Debug, Clone, Copy)]
pub enum TrayEvent {
    ToggleWindow,
    Quit,
}

struct AppTray {
    tx: async_channel::Sender<TrayEvent>,
}

impl ksni::Tray for AppTray {
    fn id(&self) -> String {
        "ca.millerfamily.SgithiAudioMeter".into()
    }

    fn icon_name(&self) -> String {
        "multimedia-volume-control".into()
    }

    fn title(&self) -> String {
        "Sgithi Audio Meter".into()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send_blocking(TrayEvent::ToggleWindow);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Show/Hide".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send_blocking(TrayEvent::ToggleWindow);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send_blocking(TrayEvent::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Start the tray icon on its own thread. Returns a receiver the GTK thread should drain (e.g.
/// via `glib::spawn_future_local`) to react to tray clicks/menu selections. A failure to start
/// (e.g. no StatusNotifierWatcher running) is logged and otherwise non-fatal - background mode
/// just won't be reachable via a tray icon, the window still works normally.
pub fn spawn() -> async_channel::Receiver<TrayEvent> {
    let (tx, rx) = async_channel::unbounded();
    if let Err(e) = (AppTray { tx }).spawn() {
        log::warn!("failed to start tray icon: {e}");
    }
    rx
}
