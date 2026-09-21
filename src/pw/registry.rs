//! Registry listener: discovers Node/Port/Link/Metadata/Factory globals and turns them into
//! `Event`s, and dispatches `Command`s that need a live proxy (Node volume/mute, Metadata
//! default sink/source) or the registry itself (link create/destroy).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use pipewire::core::CoreRc;
use pipewire::device::{Device, DeviceListener};
use pipewire::keys;
use pipewire::main_loop::MainLoopRc;
use pipewire::metadata::{Metadata, MetadataListener};
use pipewire::node::{Node, NodeListener};
use pipewire::properties::properties;
use pipewire::registry::RegistryRc;
use pipewire::spa::param::ParamType;
use pipewire::spa::utils::dict::DictRef;
use pipewire::types::ObjectType;

use crate::model::{is_hardware, Direction, ProfileOption};

use super::commands::Command;
use super::device_profile;
use super::device_route::{self, RouteVolume};
use super::events::Event;
use super::metadata as meta;
use super::node_props;
use super::peak::{self, PeakWatch};

struct BoundNode {
    node: Node,
    _listener: NodeListener,
}

struct BoundDevice {
    device: Device,
    _listener: DeviceListener,
}

pub struct PwState {
    core: CoreRc,
    registry: RegistryRc,
    event_tx: async_channel::Sender<Event>,
    nodes: HashMap<u32, BoundNode>,
    node_ids: HashSet<u32>,
    port_ids: HashSet<u32>,
    link_ids: HashSet<u32>,
    metadata: Option<(Metadata, MetadataListener)>,
    link_factory: Option<String>,
    devices: HashMap<u32, BoundDevice>,
    /// Hardware node id -> (parent PipeWire device id, that node's route-device slot), read from
    /// the node's own `device.id`/`card.profile.device` global properties.
    node_route_link: HashMap<u32, (u32, i32)>,
    /// Last known route state, keyed by (PipeWire device id, route-device slot).
    routes: HashMap<(u32, i32), RouteVolume>,
    /// Available profiles per device id, accumulated as `EnumProfile` results arrive one at a
    /// time; keyed by profile index within the inner map to dedupe re-delivered entries.
    device_profiles: HashMap<u32, HashMap<i32, ProfileOption>>,
    device_active_profile: HashMap<u32, i32>,
    /// One metering stream per node a `Strip` currently has on screen (see `pw::peak`), keyed by
    /// node id and torn down on `Command::UnwatchPeak`.
    peaks: HashMap<u32, PeakWatch>,
}

impl PwState {
    pub fn new(core: CoreRc, registry: RegistryRc, event_tx: async_channel::Sender<Event>) -> Self {
        Self {
            core,
            registry,
            event_tx,
            nodes: HashMap::new(),
            node_ids: HashSet::new(),
            port_ids: HashSet::new(),
            link_ids: HashSet::new(),
            metadata: None,
            link_factory: None,
            devices: HashMap::new(),
            node_route_link: HashMap::new(),
            routes: HashMap::new(),
            device_profiles: HashMap::new(),
            device_active_profile: HashMap::new(),
            peaks: HashMap::new(),
        }
    }
}

fn get(props: Option<&DictRef>, key: &str) -> Option<String> {
    props.and_then(|p| p.get(key)).map(str::to_owned)
}

fn get_u32(props: Option<&DictRef>, key: &str) -> Option<u32> {
    props.and_then(|p| p.get(key)).and_then(|v| v.parse().ok())
}

/// Send the current merged profile list (+ active profile, if known) for a device.
fn send_device_profiles(st: &PwState, event_tx: &async_channel::Sender<Event>, device_id: u32, active: Option<i32>) {
    let mut profiles: Vec<ProfileOption> =
        st.device_profiles.get(&device_id).map(|m| m.values().cloned().collect()).unwrap_or_default();
    profiles.sort_by_key(|p| p.index);
    let active = active.or_else(|| st.device_active_profile.get(&device_id).copied());
    let _ = event_tx.send_blocking(Event::DeviceProfilesUpdated { id: device_id, profiles, active });
}

/// Handle a newly discovered registry global. Binds a proxy for Node/Metadata (needed to send
/// commands to them later); Port/Link/Factory are read straight from their global properties.
pub fn handle_global(state: &Rc<RefCell<PwState>>, obj: &pipewire::registry::GlobalObject<&DictRef>) {
    match obj.type_ {
        ObjectType::Node => {
            // The app's own peak-metering streams (`pw::peak`) are themselves regular nodes -
            // never surface one as an application stream, or its `Strip` would start metering
            // *it*, which would start another metering stream, and so on.
            if get(obj.props, *keys::NODE_NAME).as_deref().is_some_and(|n| n.starts_with(peak::STREAM_NAME_PREFIX)) {
                return;
            }
            let Some(media_class) = get(obj.props, *keys::MEDIA_CLASS) else {
                return;
            };
            let hardware = is_hardware(&media_class);
            let name = get(obj.props, *keys::NODE_NAME).unwrap_or_else(|| format!("node-{}", obj.id));
            let description = get(obj.props, *keys::NODE_DESCRIPTION);
            // `device.id` is present in the registry's initial `global` properties, but
            // `card.profile.device` (the other half of a route link - see device_route.rs) is
            // NOT; it's only delivered later via the bound Node's `info` event. So the route link
            // can only be resolved once that first `info` event arrives, not at discovery time.
            let device_id_hint = if hardware { get_u32(obj.props, *keys::DEVICE_ID) } else { None };

            let mut st = state.borrow_mut();
            st.node_ids.insert(obj.id);
            let _ = st.event_tx.send_blocking(Event::NodeAdded {
                id: obj.id,
                name,
                description,
                media_class,
            });

            let node: Node = match st.registry.bind(obj) {
                Ok(n) => n,
                Err(e) => {
                    log::warn!("failed to bind node {}: {e}", obj.id);
                    return;
                }
            };

            let node_id = obj.id;
            // Hardware nodes' *authoritative* volume/mute comes from the parent Device's Route
            // (see the Device arm below and device_route.rs) - their own Props can independently
            // report a different, non-authoritative value (observed in practice: a Route of
            // exactly 1.0 alongside a Props of 0.91 for the same physical output). Since both
            // would otherwise race to set the same `NodeInfo.volumes` field with no precedence,
            // a route-linked node's own Props updates are ignored once the link below resolves.
            let is_route_linked = Rc::new(Cell::new(false));
            let listener = node
                .add_listener_local()
                .info({
                    let state = state.clone();
                    let is_route_linked = is_route_linked.clone();
                    move |info| {
                        let Some(device_id) = device_id_hint else { return };
                        let Some(card_profile_device) =
                            info.props().and_then(|p| p.get("card.profile.device")).and_then(|v| v.parse().ok())
                        else {
                            return;
                        };
                        let link = (device_id, card_profile_device);
                        is_route_linked.set(true);

                        let mut st = state.borrow_mut();
                        st.node_route_link.insert(node_id, link);
                        if let Some(route) = st.routes.get(&link) {
                            let _ = st.event_tx.send_blocking(Event::NodeVolumeChanged {
                                id: node_id,
                                volumes: route.volumes.clone(),
                                mute: route.mute,
                            });
                            let _ = st.event_tx.send_blocking(Event::NodePortTypeChanged {
                                id: node_id,
                                port_type: route.port_type.clone(),
                            });
                        }
                    }
                })
                .param({
                    let event_tx = st.event_tx.clone();
                    let is_route_linked = is_route_linked.clone();
                    move |_seq, param_type, _index, _next, param| {
                        if param_type != ParamType::Props || is_route_linked.get() {
                            return;
                        }
                        if let Some(param) = param {
                            if let Some((volumes, mute)) = node_props::parse_props(param) {
                                let _ = event_tx.send_blocking(Event::NodeVolumeChanged {
                                    id: node_id,
                                    volumes,
                                    mute,
                                });
                            }
                        }
                    }
                })
                .register();

            node.subscribe_params(&[ParamType::Props]);
            // `num` is bounded rather than u32::MAX: a Node's Props is always a single object,
            // and an unbounded enum request is worth avoiding given SPA_PARAM_* enumeration has
            // sharp edges in some session-manager implementations.
            node.enum_params(0, Some(ParamType::Props), 0, 8);

            st.nodes.insert(obj.id, BoundNode { node, _listener: listener });
        }
        ObjectType::Port => {
            let Some(node_id) = get_u32(obj.props, *keys::NODE_ID) else {
                return;
            };
            let name = get(obj.props, *keys::PORT_NAME).unwrap_or_else(|| format!("port-{}", obj.id));
            let direction = match get(obj.props, *keys::PORT_DIRECTION).as_deref() {
                Some("out") => Direction::Output,
                _ => Direction::Input,
            };

            let mut st = state.borrow_mut();
            st.port_ids.insert(obj.id);
            let _ = st.event_tx.send_blocking(Event::PortAdded { id: obj.id, node_id, name, direction });
        }
        ObjectType::Link => {
            let (Some(output_node), Some(output_port), Some(input_node), Some(input_port)) = (
                get_u32(obj.props, *keys::LINK_OUTPUT_NODE),
                get_u32(obj.props, *keys::LINK_OUTPUT_PORT),
                get_u32(obj.props, *keys::LINK_INPUT_NODE),
                get_u32(obj.props, *keys::LINK_INPUT_PORT),
            ) else {
                return;
            };

            let mut st = state.borrow_mut();
            st.link_ids.insert(obj.id);
            let _ = st.event_tx.send_blocking(Event::LinkAdded {
                id: obj.id,
                output_node,
                output_port,
                input_node,
                input_port,
            });
        }
        ObjectType::Metadata => {
            let is_default = get(obj.props, "metadata.name").as_deref() == Some("default");
            if !is_default {
                return;
            }

            let mut st = state.borrow_mut();
            let metadata: Metadata = match st.registry.bind(obj) {
                Ok(m) => m,
                Err(e) => {
                    log::warn!("failed to bind default metadata: {e}");
                    return;
                }
            };

            let event_tx = st.event_tx.clone();
            let listener = metadata
                .add_listener_local()
                .property(move |_subject, key, _type, value| {
                    match key {
                        Some(k) if k == meta::DEFAULT_SINK_KEY => {
                            let name = value.and_then(meta::decode_default_name);
                            let _ = event_tx.send_blocking(Event::DefaultSinkChanged { node_name: name });
                        }
                        Some(k) if k == meta::DEFAULT_SOURCE_KEY => {
                            let name = value.and_then(meta::decode_default_name);
                            let _ = event_tx.send_blocking(Event::DefaultSourceChanged { node_name: name });
                        }
                        _ => {}
                    }
                    0
                })
                .register();

            st.metadata = Some((metadata, listener));
        }
        ObjectType::Device => {
            let mut st = state.borrow_mut();
            let device: Device = match st.registry.bind(obj) {
                Ok(d) => d,
                Err(e) => {
                    log::warn!("failed to bind device {}: {e}", obj.id);
                    return;
                }
            };

            let device_id = obj.id;
            let device_name = get(obj.props, *keys::DEVICE_DESCRIPTION)
                .or_else(|| get(obj.props, *keys::DEVICE_NICK))
                .or_else(|| get(obj.props, *keys::DEVICE_NAME))
                .unwrap_or_else(|| format!("device-{device_id}"));
            let _ = st.event_tx.send_blocking(Event::DeviceAdded { id: device_id, name: device_name });

            let event_tx = st.event_tx.clone();
            let state_for_param = state.clone();
            let state_for_info = state.clone();
            let listener = device
                .add_listener_local()
                .info(move |info| {
                    // Some session-manager device implementations (bluez5 in particular, for a
                    // profile change made outside this app, e.g. a Bluetooth codec renegotiation)
                    // don't proactively push a fresh `Route`/`EnumProfile`/`Profile` param event
                    // the way `subscribe_params` normally delivers one - they only flag the change
                    // via this `info` event's `PARAMS` change mask, and expect the listener to
                    // re-enumerate. Without this, the graph's `device_active_profile`/
                    // `device_profiles` state (and so the profile dropdown) can silently go stale
                    // until the app is restarted and enumerates fresh at bind time.
                    if !info.change_mask().contains(pipewire::device::DeviceChangeMask::PARAMS) {
                        return;
                    }
                    let changed_ids: Vec<ParamType> = info.params().iter().map(|p| p.id()).collect();

                    let mut st = state_for_info.borrow_mut();
                    if changed_ids.contains(&ParamType::EnumProfile) {
                        // A fresh EnumProfile enumeration is about to start - drop whatever was
                        // accumulated last time first, or a profile that no longer exists (e.g. a
                        // Bluetooth codec that's no longer available) would linger merged into
                        // what's shown, since the `ParamType::EnumProfile` arm below only ever
                        // inserts by index and never prunes on its own.
                        st.device_profiles.remove(&device_id);
                    }
                    let Some(bound) = st.devices.get(&device_id) else { return };
                    for id in changed_ids {
                        // Bounded counts, matching the initial enum_params calls below - see the
                        // comment there on why an unbounded count is never used in this file.
                        let count = match id {
                            ParamType::Route => 32,
                            ParamType::EnumProfile => 16,
                            ParamType::Profile => 4,
                            _ => continue,
                        };
                        bound.device.enum_params(0, Some(id), 0, count);
                    }
                })
                .param(move |_seq, param_type, _index, _next, param| {
                    let Some(param) = param else { return };
                    match param_type {
                        ParamType::Route => {
                            let Some(route) = device_route::parse_route(param) else { return };

                            let mut st = state_for_param.borrow_mut();
                            let key = (device_id, route.route_device);
                            let volumes = route.volumes.clone();
                            let mute = route.mute;
                            let port_type = route.port_type.clone();
                            st.routes.insert(key, route);

                            let affected: Vec<u32> = st
                                .node_route_link
                                .iter()
                                .filter(|(_, link)| **link == key)
                                .map(|(&node_id, _)| node_id)
                                .collect();
                            drop(st);
                            for node_id in affected {
                                let _ = event_tx.send_blocking(Event::NodeVolumeChanged {
                                    id: node_id,
                                    volumes: volumes.clone(),
                                    mute,
                                });
                                let _ = event_tx.send_blocking(Event::NodePortTypeChanged {
                                    id: node_id,
                                    port_type: port_type.clone(),
                                });
                            }
                        }
                        ParamType::EnumProfile => {
                            let Some(profile) = device_profile::parse_profile(param) else { return };
                            let mut st = state_for_param.borrow_mut();
                            st.device_profiles.entry(device_id).or_default().insert(profile.index, profile);
                            send_device_profiles(&st, &event_tx, device_id, None);
                        }
                        ParamType::Profile => {
                            let Some(profile) = device_profile::parse_profile(param) else { return };
                            let mut st = state_for_param.borrow_mut();
                            st.device_active_profile.insert(device_id, profile.index);
                            send_device_profiles(&st, &event_tx, device_id, Some(profile.index));
                        }
                        _ => {}
                    }
                })
                .register();

            device.subscribe_params(&[ParamType::Route, ParamType::EnumProfile, ParamType::Profile]);
            // `num` bounded rather than u32::MAX in every case here - see the comment on the Node
            // Props enum_params call above. A real device realistically has well under a few
            // dozen routes or profiles, and exactly one currently-active profile.
            device.enum_params(0, Some(ParamType::Route), 0, 32);
            device.enum_params(0, Some(ParamType::EnumProfile), 0, 16);
            device.enum_params(0, Some(ParamType::Profile), 0, 4);

            st.devices.insert(device_id, BoundDevice { device, _listener: listener });
        }
        ObjectType::Factory => {
            if get(obj.props, *keys::FACTORY_TYPE_NAME).as_deref() == Some(ObjectType::Link.to_str()) {
                if let Some(name) = get(obj.props, *keys::FACTORY_NAME) {
                    state.borrow_mut().link_factory = Some(name);
                }
            }
        }
        _ => {}
    }
}

pub fn handle_global_remove(state: &Rc<RefCell<PwState>>, id: u32) {
    let mut st = state.borrow_mut();
    if st.node_ids.remove(&id) {
        st.nodes.remove(&id);
        st.node_route_link.remove(&id);
        let _ = st.event_tx.send_blocking(Event::NodeRemoved { id });
    } else if st.port_ids.remove(&id) {
        let _ = st.event_tx.send_blocking(Event::PortRemoved { id });
    } else if st.link_ids.remove(&id) {
        let _ = st.event_tx.send_blocking(Event::LinkRemoved { id });
    } else if st.devices.remove(&id).is_some() {
        // Drop any cached routes for this device and unlink any nodes that pointed at it, so a
        // later SetVolume/SetMute for one of those (now orphaned) nodes falls back to its own
        // Node Props instead of looking up a route that no longer exists.
        st.routes.retain(|&(device_id, _), _| device_id != id);
        st.node_route_link.retain(|_, &mut (device_id, _)| device_id != id);
        st.device_profiles.remove(&id);
        st.device_active_profile.remove(&id);
        let _ = st.event_tx.send_blocking(Event::DeviceRemoved { id });
    }
}

pub fn handle_command(state: &Rc<RefCell<PwState>>, main_loop: &MainLoopRc, cmd: Command) {
    let mut st = state.borrow_mut();
    match cmd {
        Command::WatchPeak { node_id, node_name, capture_sink } => {
            if let Some(watch) = peak::start(&st.core, &st.event_tx, node_id, &node_name, capture_sink) {
                st.peaks.insert(node_id, watch);
            }
        }
        Command::UnwatchPeak { node_id } => {
            st.peaks.remove(&node_id);
        }
        Command::SetVolume { node_id, volumes } => set_node_volume(&st, node_id, Some(&volumes), None),
        Command::SetMute { node_id, mute } => set_node_volume(&st, node_id, None, Some(mute)),
        Command::CreateLink { output_node, output_port, input_node, input_port } => {
            let Some(factory) = st.link_factory.clone() else {
                log::warn!("no link factory known yet, cannot create link");
                return;
            };
            let result = st.core.create_object::<pipewire::link::Link>(
                &factory,
                &properties! {
                    *keys::LINK_OUTPUT_NODE => output_node.to_string(),
                    *keys::LINK_OUTPUT_PORT => output_port.to_string(),
                    *keys::LINK_INPUT_NODE => input_node.to_string(),
                    *keys::LINK_INPUT_PORT => input_port.to_string(),
                    "object.linger" => "1"
                },
            );
            if let Err(e) = result {
                log::warn!("failed to create link: {e}");
            }
        }
        Command::DestroyLink { link_id } => {
            if let Err(e) = st.registry.destroy_global(link_id).into_result() {
                log::warn!("failed to destroy link {link_id}: {e}");
            }
        }
        Command::SetDefaultSink { node_name } => {
            set_default(&st, meta::DEFAULT_SINK_KEY, &node_name);
        }
        Command::SetDefaultSource { node_name } => {
            set_default(&st, meta::DEFAULT_SOURCE_KEY, &node_name);
        }
        Command::SetProfile { device_id, profile_index } => {
            let Some(bound) = st.devices.get(&device_id) else {
                log::warn!("no bound device {device_id}, cannot set profile");
                return;
            };
            let pod_bytes = device_profile::build_set_profile_pod(profile_index);
            if let Some(pod) = pipewire::spa::pod::Pod::from_bytes(&pod_bytes) {
                bound.device.set_param(ParamType::Profile, 0, pod);
            }
        }
        Command::Terminate => {
            main_loop.quit();
        }
    }
}

/// Set a node's volume/mute, routing to the parent Device's Route for hardware nodes (whose own
/// Node Props don't carry the real value - see `device_route.rs`) and to the Node's own Props
/// for everything else (software sinks/sources and application streams).
fn set_node_volume(st: &PwState, node_id: u32, volumes: Option<&[f32]>, mute: Option<bool>) {
    if let Some(&(device_id, route_device)) = st.node_route_link.get(&node_id) {
        let Some(route) = st.routes.get(&(device_id, route_device)) else {
            log::warn!("no known route yet for node {node_id}, ignoring volume/mute change");
            return;
        };
        let Some(bound) = st.devices.get(&device_id) else {
            return;
        };
        let pod_bytes = device_route::build_route_pod(route.index, route_device, volumes, mute);
        if let Some(pod) = pipewire::spa::pod::Pod::from_bytes(&pod_bytes) {
            bound.device.set_param(ParamType::Route, 0, pod);
        }
        return;
    }

    if let Some(bound) = st.nodes.get(&node_id) {
        let pod_bytes = node_props::build_props_pod(volumes, mute);
        if let Some(pod) = pipewire::spa::pod::Pod::from_bytes(&pod_bytes) {
            bound.node.set_param(ParamType::Props, 0, pod);
        }
    }
}

fn set_default(st: &PwState, key: &str, node_name: &str) {
    let Some((metadata, _)) = &st.metadata else {
        log::warn!("no default metadata object bound yet");
        return;
    };
    metadata.set_property(
        pipewire::core::PW_ID_CORE,
        key,
        Some(meta::JSON_TYPE),
        Some(&meta::encode_default_value(node_name)),
    );
}
