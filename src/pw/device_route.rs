//! Hardware devices (real ALSA sinks/sources, as opposed to software/stream nodes) don't carry
//! their volume/mute on the Node's own `Props` - PipeWire leaves that at `0.0`/unset for them.
//! The actual hardware volume lives on the parent `Device`'s `SPA_PARAM_Route`, keyed by a
//! `(route index, route device)` pair. This mirrors what `wpctl`/`pavucontrol` read and write.

use pipewire::spa::param::ParamType;
use pipewire::spa::pod::{deserialize::PodDeserializer, Pod, Property, Value};
use pipewire::spa::sys as spa_sys;
use pipewire::spa::utils::SpaTypes;

use super::node_props;

#[derive(Debug, Clone)]
pub struct RouteVolume {
    pub index: i32,
    /// The route's own `device` field - a route slot index scoped to the PipeWire `Device`, not
    /// to be confused with the PipeWire object id of that `Device`. This is what a Node's
    /// `card.profile.device` property links back to.
    pub route_device: i32,
    pub volumes: Vec<f32>,
    pub mute: bool,
    /// The route's `port.type` info key (e.g. `"mic"`, `"line"`, `"headset-mic"`), when present -
    /// the precise signal `model::NodeInfo::is_mic_like()` prefers over its display-name
    /// heuristic.
    pub port_type: Option<String>,
}

/// `SPA_PARAM_ROUTE_info` is a `Struct(Int: n_items, (String: key, String: value)*)` - `n_items`
/// counts *pairs*, not raw struct fields. Extract `port.type` if present.
fn parse_route_info(items: &[Value]) -> Option<String> {
    let mut pairs = items.iter();
    pairs.next()?; // n_items count - unneeded, we just walk pairs until they run out
    loop {
        let (Some(key), Some(value)) = (pairs.next(), pairs.next()) else { return None };
        if let (Value::String(key), Value::String(value)) = (key, value) {
            if key == "port.type" {
                return Some(value.clone());
            }
        }
    }
}

/// Parse an incoming `Route` param pod (as delivered by a `Device`'s `param` event).
pub fn parse_route(pod: &Pod) -> Option<RouteVolume> {
    let (_, value) = PodDeserializer::deserialize_from::<Value>(pod.as_bytes()).ok()?;
    let Value::Object(object) = value else {
        return None;
    };

    let mut index = None;
    let mut route_device = None;
    let mut volumes = Vec::new();
    let mut mute = false;
    let mut port_type = None;

    for prop in &object.properties {
        if prop.key == spa_sys::SPA_PARAM_ROUTE_index {
            if let Value::Int(i) = prop.value {
                index = Some(i);
            }
        } else if prop.key == spa_sys::SPA_PARAM_ROUTE_device {
            if let Value::Int(i) = prop.value {
                route_device = Some(i);
            }
        } else if prop.key == spa_sys::SPA_PARAM_ROUTE_props {
            if let Value::Object(props) = &prop.value {
                if let Some((v, m)) = node_props::extract_volume_mute(&props.properties) {
                    volumes = v;
                    mute = m;
                }
            }
        } else if prop.key == spa_sys::SPA_PARAM_ROUTE_info {
            if let Value::Struct(items) = &prop.value {
                port_type = parse_route_info(items);
            }
        }
    }

    Some(RouteVolume { index: index?, route_device: route_device?, volumes, mute, port_type })
}

/// Build a `Route` pod applying a volume/mute change to a specific route, for `Device::set_param`.
pub fn build_route_pod(index: i32, route_device: i32, volumes: Option<&[f32]>, mute: Option<bool>) -> Vec<u8> {
    let props = node_props::props_object(volumes, mute);
    let value = Value::Object(pipewire::spa::pod::Object {
        type_: SpaTypes::ObjectParamRoute.as_raw(),
        id: ParamType::Route.as_raw(),
        properties: vec![
            Property::new(spa_sys::SPA_PARAM_ROUTE_index, Value::Int(index)),
            Property::new(spa_sys::SPA_PARAM_ROUTE_device, Value::Int(route_device)),
            Property::new(spa_sys::SPA_PARAM_ROUTE_props, Value::Object(props)),
            Property::new(spa_sys::SPA_PARAM_ROUTE_save, Value::Bool(true)),
        ],
    });
    node_props::serialize(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_port_type_from_route_info() {
        let items = vec![
            Value::Int(2), // n_items (pairs, not raw fields)
            Value::String("port.type".into()),
            Value::String("mic".into()),
            Value::String("other.key".into()),
            Value::String("other.value".into()),
        ];
        assert_eq!(parse_route_info(&items), Some("mic".to_string()));
    }

    #[test]
    fn route_info_without_port_type_returns_none() {
        let items = vec![Value::Int(1), Value::String("other.key".into()), Value::String("other.value".into())];
        assert_eq!(parse_route_info(&items), None);
    }

    #[test]
    fn round_trips_index_device_volume_and_mute() {
        // 0.404306 is a real Route channelVolumes value seen via pw-dump (see `ui::volume`).
        let bytes = build_route_pod(2, 0, Some(&[0.404306]), Some(false));
        let pod = Pod::from_bytes(&bytes).expect("valid pod");
        let route = parse_route(pod).expect("parses");
        assert_eq!(route.index, 2);
        assert_eq!(route.route_device, 0);
        assert_eq!(route.volumes, vec![0.404306]);
        assert!(!route.mute);
        // build_route_pod doesn't set ROUTE_info - only real Device param events carry port.type.
        assert_eq!(route.port_type, None);
    }
}
