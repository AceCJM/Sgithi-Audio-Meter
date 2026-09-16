# Sgithi Audio Meter

A GTK4 desktop app for controlling [PipeWire](https://pipewire.org/) audio on Linux, combining:

- **[pavucontrol](https://github.com/pulseaudio/pavucontrol)**-style volume/mute control for hardware devices and application streams, plus default sink/source selection.
- **Voicemeeter**-style layout: everything shown together on one page as vertical mixing-console fader strips, instead of separate tabs.
- **[qpwgraph](https://github.com/rncbc/qpwgraph)**-style patchbay: a node-graph view for manually linking/unlinking any port to any other port.

## Status

Early but functional. v1 controls and routes *existing* PipeWire devices and streams; it does not create virtual sinks/cables (see `BACKLOG.md`).

## Features

**Mixer page** — two columns, each a set of vertical fader strips:
- *Inputs*: hardware capture devices (`Audio/Source`) and application playback streams (pavucontrol's "Playback" tab).
- *Outputs*: hardware playback devices (`Audio/Sink`) and application recording streams (pavucontrol's "Recording" tab).

Each strip has a volume fader, a mute toggle, and — for hardware devices — a "Set Default" button. Hardware volume is read and written via the parent PipeWire `Device`'s `Route` param (where ALSA devices actually keep it), not the Node's own `Props`.

**Patchbay page** — a hand-drawn node-graph canvas:
- Drag a node's body to reposition it.
- Drag from a port to draw a link; release over a compatible port (opposite direction, different node) to create it.
- Right-click a link to remove it.
- Hovering a link highlights it.

## Building and running

Requires GTK4 and PipeWire development headers, plus a C compiler toolchain (for `bindgen`, used by the `pipewire`/`libspa` crates to generate FFI bindings at build time):

```sh
sudo apt install build-essential pkg-config libgtk-4-dev libpipewire-0.3-dev libglib2.0-dev libclang-dev
```

Then:

```sh
cargo run
```

There's no packaging yet (no `.desktop` file or install step) — see `BACKLOG.md`.

## Architecture

Rust + [gtk4-rs](https://gtk-rs.org/) + [pipewire-rs](https://gitlab.freedesktop.org/pipewire/pipewire-rs). PipeWire runs on its own OS thread (`src/pw/`) and talks to the GTK thread (`src/ui/`) over channels — PipeWire callbacks are never touched from GTK code and vice versa. `src/model.rs` holds the shared, plain-data view of the PipeWire graph that both the mixer and patchbay pages render from.

```
src/
  model.rs           # shared graph state: nodes, ports, links, defaults
  pw/                 # PipeWire integration, on its own thread
    thread.rs           # owns the PipeWire main loop
    registry.rs           # discovers Node/Port/Link/Device/Metadata globals
    node_props.rs           # software node volume/mute (SPA_PARAM_Props)
    device_route.rs           # hardware device volume/mute (SPA_PARAM_Route)
    metadata.rs                # default sink/source (PipeWire Metadata)
    commands.rs / events.rs      # UI <-> PipeWire thread messages
  ui/                 # GTK4 widgets
    app.rs               # window, navigation, event loop
    mixer/                  # fader-strip mixer page
    patchbay/                 # node-graph canvas page
```

## A note on testing against a live PipeWire session

This app talks directly to your real, running PipeWire/WirePlumber session — there's no sandbox. Be cautious with anything that calls `enum_params` with a large/unbounded count; see `BACKLOG.md`'s notes and the project's Claude memory for a past incident where this stalled WirePlumber system-wide.
