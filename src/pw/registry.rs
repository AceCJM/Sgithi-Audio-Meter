//! Registry listener: discovers Node/Port/Link/Metadata/Factory globals and turns them into
//! `Event`s, and dispatches `Command`s that need a live proxy (Node volume/mute, Metadata
//! default sink/source) or the registry itself (link create/destroy).

use std::cell::RefCell;
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

use crate::model::{is_hardware, Direction};

use super::commands::Command;
use super::device_route::{self, RouteVolume};
use super::events::Event;
use super::metadata as meta;
use super::node_props;

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
        }
    }
}

fn get(props: Option<&DictRef>, key: &str) -> Option<String> {
    props.and_then(|p| p.get(key)).map(str::to_owned)
}

fn get_u32(props: Option<&DictRef>, key: &str) -> Option<u32> {
    props.and_then(|p| p.get(key)).and_then(|v| v.parse().ok())
}

fn get_i32(props: Option<&DictRef>, key: &str) -> Option<i32> {
    props.and_then(|p| p.get(key)).and_then(|v| v.parse().ok())
}

/// Handle a newly discovered registry global. Binds a proxy for Node/Metadata (needed to send
/// commands to them later); Port/Link/Factory are read straight from their global properties.
pub fn handle_global(state: &Rc<RefCell<PwState>>, obj: &pipewire::registry::GlobalObject<&DictRef>) {
    match obj.type_ {
        ObjectType::Node => {
            let Some(media_class) = get(obj.props, *keys::MEDIA_CLASS) else {
                return;
            };
            let hardware = is_hardware(&media_class);
            let name = get(obj.props, *keys::NODE_NAME).unwrap_or_else(|| format!("node-{}", obj.id));
            let description = get(obj.props, *keys::NODE_DESCRIPTION);
            let route_link = if hardware {
                get_u32(obj.props, *keys::DEVICE_ID).zip(get_i32(obj.props, "card.profile.device"))
            } else {
                None
            };

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
            let event_tx = st.event_tx.clone();
            let listener = node
                .add_listener_local()
                .param(move |_seq, param_type, _index, _next, param| {
                    if param_type != ParamType::Props {
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
                })
                .register();

            node.subscribe_params(&[ParamType::Props]);
            // `num` is bounded rather than u32::MAX: a Node's Props is always a single object,
            // and an unbounded enum request is worth avoiding given SPA_PARAM_* enumeration has
            // sharp edges in some session-manager implementations.
            node.enum_params(0, Some(ParamType::Props), 0, 8);

            st.nodes.insert(obj.id, BoundNode { node, _listener: listener });

            // Hardware sinks/sources don't carry real volume/mute on their own Props (PipeWire
            // leaves that at 0.0) - the actual value lives on the parent Device's Route. Link
            // this node to its route slot, and if that route's state is already known (the
            // Device may have been discovered before this Node), emit it immediately instead of
            // leaving the strip at a misleading 0.0 until a fresh Route event happens to arrive.
            if let Some(link) = route_link {
                st.node_route_link.insert(node_id, link);
                if let Some(route) = st.routes.get(&link) {
                    let _ = st.event_tx.send_blocking(Event::NodeVolumeChanged {
                        id: node_id,
                        volumes: route.volumes.clone(),
                        mute: route.mute,
                    });
                }
            }
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
            let event_tx = st.event_tx.clone();
            let state_for_route = state.clone();
            let listener = device
                .add_listener_local()
                .param(move |_seq, param_type, _index, _next, param| {
                    if param_type != ParamType::Route {
                        return;
                    }
                    let Some(param) = param else { return };
                    let Some(route) = device_route::parse_route(param) else { return };

                    let mut st = state_for_route.borrow_mut();
                    let key = (device_id, route.route_device);
                    let volumes = route.volumes.clone();
                    let mute = route.mute;
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
                    }
                })
                .register();

            device.subscribe_params(&[ParamType::Route]);
            // `num` bounded rather than u32::MAX - see the comment on the Node Props enum_params
            // call above. A real device realistically has well under a few dozen routes.
            device.enum_params(0, Some(ParamType::Route), 0, 32);

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
    }
}

pub fn handle_command(state: &Rc<RefCell<PwState>>, main_loop: &MainLoopRc, cmd: Command) {
    let st = state.borrow();
    match cmd {
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
