# Sgithi Audio Meter

A GTK4 desktop app for controlling [PipeWire](https://pipewire.org/) audio on Linux, combining:

- **[pavucontrol](https://github.com/pulseaudio/pavucontrol)**-style volume/mute control for hardware devices and application streams, plus default sink/source selection.
- **Voicemeeter**-style layout: everything shown together on one page as vertical mixing-console fader strips, instead of separate tabs.
- **[qpwgraph](https://github.com/rncbc/qpwgraph)**-style patchbay: a node-graph view for manually linking/unlinking any port to any other port.

## Status

Early but functional. v1 controls and routes *existing* PipeWire devices and streams; it does not create virtual sinks/cables (see `BACKLOG.md`).

## Features

Three tabs: **Devices**, **Applications**, **Patchbay**.

**Devices page** — a per-device profile bar, then two columns of vertical fader strips for every hardware `Audio/Sink`/`Audio/Source`:
- A **profile dropdown** per device that has more than one selectable profile (e.g. "Off" / "Analog Stereo Duplex" / "Pro Audio" for an audio interface).
- *Inputs*, split into **Microphones** and **Other Inputs** (e.g. line/monitor inputs) - classified by display name by default, overridable per-device (see below).
- *Outputs*: every hardware playback device.

**Applications page** — every application stream (nothing hardware), as vertical fader strips in two columns:
- *Playback*: apps producing sound (pavucontrol's "Playback" tab).
- *Recording*: apps capturing audio, e.g. a video call's microphone use (pavucontrol's "Recording" tab).

On both pages, each strip has a volume fader (0-150%, cubic/perceptual scale matching pavucontrol/wpctl, styled red above 100%), a mute toggle, a **✎ rename/re-categorize** button (custom display name, and for hardware inputs a Microphone/Other Input override - both persisted to `~/.config/sgithi-audio-meter/node-overrides.json`), and — for hardware devices only — a "Set Default" button. Hardware volume is read and written via the parent PipeWire `Device`'s `Route` param (where ALSA devices actually keep it), not the Node's own `Props`.

**Patchbay page** — a hand-drawn node-graph canvas:
- Drag a node's body to reposition it - positions are saved (by node name, to `~/.config/sgithi-audio-meter/patchbay-layout.json`) and restored on the next launch.
- Drag from a port to draw a link; release over a compatible port (opposite direction, different node) to create it.
- Right-click a link to remove it; right-click a node to hide it; right-click empty canvas to bring back a hidden node. Hidden state persists too (`patchbay-hidden.json`).
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

### Installing (.desktop entry + icon)

```sh
packaging/install.sh              # builds a release binary, installs to ~/.local/{bin,share} - no sudo
packaging/install.sh --system     # installs to /usr/local instead, uses sudo
packaging/install.sh --uninstall  # removes it again (add --system to match a system-wide install)
```

Pre-built binaries are also published as GitHub Actions release artifacts on tagged releases (see `.github/workflows/release.yml`).

### Developed and tested on

| | |
|---|---|
| OS | Debian GNU/Linux 13 (trixie) |
| Kernel | Linux 6.12 (x86_64) |
| Desktop | XFCE, X11 |
| CPU | Intel Core i5-13400F |
| Rust | rustc/cargo 1.97.1 |
| GTK4 | 4.18.6 |
| PipeWire / WirePlumber | 1.4.2 |
| Audio hardware | Focusrite Scarlett Solo (4th Gen, USB) as the primary sink/source under test, Logitech PRO X 2 LIGHTSPEED as a secondary sink |

Other distros/desktops/PipeWire versions should work the same (nothing here is Debian- or XFCE-specific), but this is the only combination it's actually been run and verified against so far.

## Architecture

Rust + [gtk4-rs](https://gtk-rs.org/) + [pipewire-rs](https://gitlab.freedesktop.org/pipewire/pipewire-rs). PipeWire runs on its own OS thread (`src/pw/`) and talks to the GTK thread (`src/ui/`) over channels — PipeWire callbacks are never touched from GTK code and vice versa. `src/model.rs` holds the shared, plain-data view of the PipeWire graph that all three pages render from.

```
src/
  model.rs           # shared graph state: nodes, ports, links, devices, defaults
  pw/                 # PipeWire integration, on its own thread
    thread.rs           # owns the PipeWire main loop
    registry.rs           # discovers Node/Port/Link/Device/Metadata globals
    node_props.rs           # software node volume/mute (SPA_PARAM_Props)
    device_route.rs           # hardware device volume/mute (SPA_PARAM_Route)
    device_profile.rs          # device profile list + active profile (SPA_PARAM_Profile)
    metadata.rs                # default sink/source (PipeWire Metadata)
    commands.rs / events.rs      # UI <-> PipeWire thread messages
  ui/                 # GTK4 widgets
    app.rs               # window, navigation, event loop
    strip.rs               # one fader strip, shared by Devices & Applications
    overrides.rs             # per-node rename/re-categorize, persisted
    volume.rs                 # linear <-> perceptual/cubic volume conversion
    devices/                    # Devices page + per-device profile bar
    applications/                 # Applications page (app streams only)
    patchbay/                       # node-graph canvas page + layout/hidden-node persistence
```

## A note on testing against a live PipeWire session

This app talks directly to your real, running PipeWire/WirePlumber session — there's no sandbox. Be cautious with anything that calls `enum_params` with a large/unbounded count; see `BACKLOG.md`'s notes and the project's Claude memory for a past incident where this stalled WirePlumber system-wide.
