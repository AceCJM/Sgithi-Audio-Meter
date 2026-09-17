//! Owns the PipeWire main loop on a dedicated OS thread.
//!
//! GTK widgets are not thread-safe, so PipeWire's callback-driven API is never touched from the
//! GTK thread. Instead this thread turns PipeWire callbacks into `Event`s sent over an
//! `async_channel` to the GTK thread, and receives `Command`s from the GTK thread over a
//! `pipewire::channel` attached to this loop.
//!
//! If the connection to the PipeWire daemon drops (e.g. `pipewire`/`wireplumber` restarting),
//! this thread sends `Event::Disconnected` and exits rather than attempting a live reconnect: the
//! `Command` `Receiver` is consumed by `Receiver::attach`, permanently tied to this thread's
//! `MainLoopRc` - reconnecting would mean building a new channel and re-pointing every `Command`
//! sender already cloned into every page and `Strip`, a bigger change than this pass attempts.
//! The user needs to restart the app after a disconnect.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::commands::Command;
use super::events::Event;
use super::registry::{handle_command, handle_global, handle_global_remove, PwState};

/// Spawn the PipeWire thread. Returns a sender the UI thread can use to issue commands, and an
/// async_channel receiver the UI thread should drain (e.g. via `glib::spawn_future_local`) to
/// apply `Event`s to the shared `model::Graph`.
pub fn spawn() -> (pipewire::channel::Sender<Command>, async_channel::Receiver<Event>) {
    let (cmd_tx, cmd_rx) = pipewire::channel::channel::<Command>();
    let (event_tx, event_rx) = async_channel::unbounded::<Event>();

    std::thread::Builder::new()
        .name("pipewire".to_string())
        .spawn(move || {
            if let Err(e) = run(event_tx, cmd_rx) {
                log::error!("pipewire thread exited with error: {e}");
            }
        })
        .expect("failed to spawn pipewire thread");

    (cmd_tx, event_rx)
}

fn run(
    event_tx: async_channel::Sender<Event>,
    cmd_rx: pipewire::channel::Receiver<Command>,
) -> Result<(), pipewire::Error> {
    pipewire::init();

    let main_loop = pipewire::main_loop::MainLoopRc::new(None)?;
    let context = pipewire::context::ContextRc::new(&main_loop, None)?;
    let core = context.connect_rc(None)?;
    let registry = core.get_registry_rc()?;

    let state = Rc::new(RefCell::new(PwState::new(core.clone(), registry.clone(), event_tx.clone())));

    let main_loop_for_cmds = main_loop.clone();
    let state_for_cmds = state.clone();
    let _cmd_receiver = cmd_rx.attach(main_loop.loop_(), move |cmd| {
        handle_command(&state_for_cmds, &main_loop_for_cmds, cmd);
    });

    let state_for_global = state.clone();
    let state_for_remove = state.clone();
    let _registry_listener = registry
        .add_listener_local()
        .global(move |obj| handle_global(&state_for_global, obj))
        .global_remove(move |id| handle_global_remove(&state_for_remove, id))
        .register();

    // A core-level error (id == PW_ID_CORE) means the connection itself is gone, not just one
    // object - e.g. the daemon restarting. Quit the loop so `run()` can report it below, instead
    // of leaving the app silently stuck showing stale state forever.
    let disconnected = Rc::new(Cell::new(false));
    let _core_listener = {
        let main_loop = main_loop.clone();
        let disconnected = disconnected.clone();
        core.add_listener_local()
            .error(move |id, _seq, res, message| {
                if id == pipewire::core::PW_ID_CORE {
                    log::error!("pipewire core error (res={res}): {message}");
                    disconnected.set(true);
                    main_loop.quit();
                }
            })
            .register()
    };

    main_loop.run();

    if disconnected.get() {
        let _ = event_tx.send_blocking(Event::Disconnected);
    }

    Ok(())
}
