# Backlog

Future work not in v1, roughly ordered by priority within each section.

## Near-term polish

- **Peak/level meters.** Deferred from v1 on purpose (see `plan` history) — real live peak metering needs a `pipewire::stream::Stream` per visible node, connected to its monitor port, computing peak from raw samples in the `process` callback (this is what pavucontrol/qpwgraph actually do), throttled to ~30Hz before posting to the GTK thread. One extra `Stream` object per mixer strip, with its own real-time callback — get this right in isolation before wiring into the UI. The strip widget already has a slot reserved for this.
- **Port/profile selection.** pavucontrol has a dropdown per hardware device for its active port/profile ("Analog Stereo Duplex", etc.), backed by `Device`'s `EnumRoute`/`Profile`/`EnumProfile` params. Not implemented at all yet — currently the app only reads/writes the *volume* half of `Route`, not profile switching.
- **Volume curve.** Faders are currently linear over 0.0–1.5. pavucontrol/wpctl apply a cubic curve for perceptual volume — worth matching so the fader "feels" the same as other tools, and so displayed percentages line up with `wpctl get-volume`.
- **Over-amplification indicator.** pavucontrol highlights the fader red above 100%. Nice, cheap addition once the curve above is sorted.
- **Device removal cleanup.** `PwState` doesn't currently prune `devices`/`routes` entries when a `Device` global is removed (only Node/Port/Link removal is handled in `handle_global_remove`). Rare in practice (unplugging hardware) but worth closing for correctness.

## Medium-term features

- **Virtual audio cables.** Voicemeeter's signature feature — creating/destroying PipeWire `null-sink` "virtual cable" devices on demand from the UI. Explicitly out of scope for v1 (confirmed with the user during planning); v1 only controls/routes what already exists. Would need module load/unload management and probably persistence across restarts.
- **Patchbay layout persistence.** Node positions in the patchbay are in-memory only (`CanvasModel`), reset every launch. Save to a config file keyed by node name (ids aren't stable across restarts).
- **Patchbay pan/zoom.** Canvas is currently a fixed-size `DrawingArea` in a `ScrolledWindow`; a `GestureZoom`/scroll-to-zoom controller would help once real multi-device graphs get visually crowded.
- **Better auto-layout.** The current column heuristic (outputs-only left, inputs-only right, everything else in a middle column) is a first guess, not validated against large/complex graphs.
- **Packaging.** No `.desktop` file, icon, or install step yet — `cargo run` only. Add a Meson wrapper or plain install script, an icon, and a `.desktop` entry once the app is stable enough to want on a system menu.
- **libadwaita styling.** Currently plain `gtk::HeaderBar`/`gtk::Stack` to minimize dependencies. Swapping to `AdwApplicationWindow`/`AdwViewSwitcher` for GNOME-native styling is an isolated change to `src/ui/app.rs` plus the `libadwaita-1-dev` apt package.
- **Per-channel / balance control.** A strip's fader currently sets every channel to the same value. No balance/pan control for multi-channel devices.

## Longer-term / stretch

- **Automated tests.** None yet. `model::Graph::apply` and the `pw::node_props`/`pw::device_route` pod (de)serialization are the most testable, PipeWire-independent pieces — a good place to start (e.g. round-trip a known pod byte sequence).
- **CI.** No CI configured. Given the app needs system dev headers to even compile (`libgtk-4-dev`, `libpipewire-0.3-dev`, `libclang-dev`), a CI image would need those installed.
- **Reconnect handling.** If the PipeWire connection drops (daemon restart), the app doesn't currently attempt to reconnect — it would just go stale.
- **Search/filter.** No filtering for the mixer page; fine for a handful of devices/streams, would get unwieldy with many concurrent app streams.
- **Tray icon / background mode.** Voicemeeter typically runs minimized to the tray; this app currently has no such mode.

## Standing constraint: live PipeWire testing safety

Not a feature, but worth keeping visible: **never call `enum_params` with an unbounded count (`u32::MAX`)** against a live PipeWire/WirePlumber session. Early hardware-volume (`Device`/`Route`) work did exactly this and left the user's real `wireplumber` process stuck at 100% CPU system-wide, requiring a service restart to recover. The current code bounds these calls (8 for Node `Props`, 32 for Device `Route`); keep that pattern for any new param-enumeration code, and be cautious re-testing PipeWire-touching changes repeatedly against a live session — a CPU watchdog on `wireplumber` alongside any test launch is cheap insurance.
