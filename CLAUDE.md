# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A GTK4 desktop app for controlling PipeWire audio on Linux, combining pavucontrol-style volume/mute/default-device control, a Voicemeeter-style unified mixer page (hardware devices and app streams as vertical fader strips, Inputs/Outputs columns on one page), and a qpwgraph-style patchbay for manually linking/unlinking ports. See `README.md` for feature details and `BACKLOG.md` for what's intentionally not built yet.

v1 only controls/routes *existing* PipeWire devices and streams - no virtual sink/cable creation.

## Commands

Requires system dev headers before anything will build (not installable by Claude - has no sudo):
```sh
sudo apt install build-essential pkg-config libgtk-4-dev libpipewire-0.3-dev libglib2.0-dev libclang-dev
```

```sh
cargo build      # build
cargo run        # build and launch
cargo check      # fast type-check without codegen
```

```sh
cargo test       # runs the (currently small) unit test suite, e.g. src/ui/volume.rs
```

No rustfmt/clippy config, no CI - plain `cargo`/`cargo test`/`cargo clippy` defaults only.

## Architecture

Rust + `gtk4-rs` (crate is `gtk4`, aliased to `gtk` in `Cargo.toml` via `gtk = { package = "gtk4" }` - import as `use gtk::...`, not `gtk4::...`) + `pipewire-rs`/`libspa` (accessed as `pipewire::spa`; no separate `libspa` dependency needed, it's re-exported).

**Thread split**: PipeWire's callback-driven API runs entirely on its own OS thread (`src/pw/thread.rs` owns the `MainLoopRc`); GTK widgets are not thread-safe and are never touched from that thread. The two sides only communicate through:
- `pw::Event` (PipeWire thread -> GTK thread), sent over an `async_channel`, drained on the GTK thread via `glib::spawn_future_local` in `src/ui/app.rs`.
- `pw::Command` (GTK thread -> PipeWire thread), sent over a `pipewire::channel` attached to the PipeWire main loop.

`src/model.rs` (`Graph`) is the plain-data, PipeWire/GTK-agnostic view of the current state (nodes, ports, links, default sink/source) that both UI pages render from and that `Event`s get applied to (`Graph::apply`).

**`src/pw/`** (PipeWire integration, never imports `gtk`):
- `registry.rs` - the core: `PwState` + the registry `global`/`global_remove` listener that discovers `Node`/`Port`/`Link`/`Device`/`Metadata`/`Factory` objects and turns them into `Event`s, plus `handle_command` which dispatches `Command`s.
- `node_props.rs` - builds/parses the generic `SPA_TYPE_OBJECT_Props` pod (`SPA_PROP_channelVolumes`/`SPA_PROP_mute`) used for **software** node volume/mute (app streams, software sinks). libspa has no typed wrapper for these generic prop keys, so raw `spa_sys::SPA_PROP_*` constants are used directly.
- `device_route.rs` - **hardware** sink/source volume/mute does *not* reliably live on the Node's own Props (observed both as `0.0` and as a stale/differing value vs. the real setting) - the authoritative value lives on the parent `Device`'s `SPA_PARAM_Route`, keyed by `(route index, route "device" slot)`. A hardware Node links to its route via `device.id` (-> the PipeWire `Device` object id) and `card.profile.device` (-> the route's `device` field - confusingly *not* the same number as the PipeWire device id). **`card.profile.device` is only available from the bound Node's `.info()` event, not the registry's initial `global` event props** - this is a general trap (see "standing lessons" below), not specific to this one key. `registry.rs`'s Node arm resolves the link inside its `.info()` callback (not at discovery time) and uses an `Rc<Cell<bool>>` to make its `.param()` callback stop trusting the Node's own Props once the link resolves; `set_node_volume` picks Route-vs-Props per node the same way.
- `metadata.rs` - default sink/source read (`Metadata` `property` events) and write (`Metadata::set_property`), keys `default.audio.sink`/`default.audio.source`, JSON-encoded `{"name": "<node.name>"}`.
- `commands.rs` / `events.rs` - the two message enums crossing the thread boundary.

**`src/ui/`**:
- `app.rs` - window/nav shell (plain `gtk::HeaderBar` + `gtk::Stack`, not libadwaita, to keep deps minimal) and the event-loop closure that applies `Event`s to `Graph` and calls `sync()` on both pages.
- `mixer/` - `page.rs` reconciles `Strip` widgets (create/destroy on node add/remove, `update()` on volume/mute change) against `Graph`, keyed by node id so a slider isn't reset mid-drag; `strip.rs` is one fader strip. `Strip::update()` blocks the fader/mute `SignalHandlerId`s around its programmatic `set_value`/`set_active` calls - see "standing lessons" below for why that's load-bearing, not just tidy.
- `volume.rs` - linear (PipeWire wire format) <-> perceptual/cubic (displayed, matches pavucontrol/wpctl) conversion. Has unit tests (`cargo test`).
- `patchbay/` - hand-drawn `gtk::DrawingArea` canvas (no mature GTK4 node-graph widget crate exists). `canvas_model.rs` does node auto-layout (columns by port direction) and hit-testing (`node_at`/`port_at`/`link_at`, the last via bezier-curve point sampling); `render.rs` draws with `cairo`; `page.rs` wires `GestureDrag`/`GestureClick`/`EventControllerMotion` to it. Link creation is never drawn optimistically - the UI waits for the real `Event::LinkAdded` round-trip from the registry, since PipeWire object creation is async.

## Standing lessons

There is no sandbox - `cargo run` connects to whatever PipeWire/WirePlumber session is actually running on the machine, and commands sent from the mixer/patchbay genuinely change the user's real audio routing and volumes. These came out of real incidents/bugs during development (see `BACKLOG.md`'s last section for the fuller writeup):

- **Never call `enum_params` with an unbounded count (`u32::MAX`).** Doing this for `Device`/`Route` enumeration once left the user's real `wireplumber` process stuck at 100% CPU indefinitely, hanging every PipeWire-dependent command system-wide until the service was restarted. Current code uses small bounded counts (8 for Node `Props`, 32 for Device `Route`) - keep that pattern for any new param-enumeration code.
- Before repeatedly re-launching the app to test PipeWire-touching changes, especially after any prior hang/timeout, run a CPU watchdog on the `wireplumber` process (`ps -o %cpu= -p <pid>` polled every ~0.5s) alongside the test launch and kill the test process immediately if CPU spikes and stays high, rather than iterating blindly.
- **A registry global's `props` (from the `global` listener) is a smaller set than a bound object's `.info().props()`.** Don't assume a property is present in `GlobalObject.props` just because `pw-dump` shows it - `pw-dump` reads the fuller `info().props()`. If a key seems to be missing where you expect it, check whether it only shows up after binding and listening for `.info()`.
- **Blindly calling a widget's `set_value`/`set_active` while its `connect_*` handler is still attached can silently re-send a command.** For this app that means writing back to the user's real audio hardware for a change they never made. Any widget that both displays PipeWire state and accepts user input needs its handler blocked around programmatic updates (see `Strip::update()`).
