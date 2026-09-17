//! Building/parsing the generic `SPA_TYPE_OBJECT_Props` pod (`ParamType::Props`), used both
//! standalone (software nodes' own volume/mute) and nested inside a `Route` pod (hardware
//! devices' volume/mute - see `device_route.rs`). libspa doesn't expose typed wrappers for these
//! generic prop keys (only for Format), so we build/parse `Object`/`Property` values directly
//! using the raw `SPA_PROP_*` key constants from `spa_sys`.

use pipewire::spa::param::ParamType;
use pipewire::spa::pod::{deserialize::PodDeserializer, serialize::PodSerializer, Object, Pod, Property, Value, ValueArray};
use pipewire::spa::sys as spa_sys;
use pipewire::spa::utils::SpaTypes;
use std::io::Cursor;

/// Build a `Value::Object` representing `SPA_TYPE_OBJECT_Props` with the given channel volumes
/// and/or mute state set. Pass `None` for a field to leave it unspecified.
pub fn props_object(volumes: Option<&[f32]>, mute: Option<bool>) -> Object {
    let mut properties = Vec::new();
    if let Some(volumes) = volumes {
        properties.push(Property::new(
            spa_sys::SPA_PROP_channelVolumes,
            Value::ValueArray(ValueArray::Float(volumes.to_vec())),
        ));
    }
    if let Some(mute) = mute {
        properties.push(Property::new(spa_sys::SPA_PROP_mute, Value::Bool(mute)));
    }
    Object { type_: SpaTypes::ObjectParamProps.as_raw(), id: ParamType::Props.as_raw(), properties }
}

/// Serialize a `Value` (an `Object`, typically) into raw pod bytes.
pub fn serialize(value: &Value) -> Vec<u8> {
    let (cursor, _) = PodSerializer::serialize(Cursor::new(Vec::new()), value).expect("failed to serialize pod");
    cursor.into_inner()
}

/// Build a standalone `SPA_TYPE_OBJECT_Props` pod for `Node::set_param`.
pub fn build_props_pod(volumes: Option<&[f32]>, mute: Option<bool>) -> Vec<u8> {
    serialize(&Value::Object(props_object(volumes, mute)))
}

/// Extract `(channel volumes, mute)` from a Props object's properties, if present.
pub fn extract_volume_mute(properties: &[Property]) -> Option<(Vec<f32>, bool)> {
    let mut volumes = None;
    let mut mute = None;
    for prop in properties {
        if prop.key == spa_sys::SPA_PROP_channelVolumes {
            if let Value::ValueArray(ValueArray::Float(v)) = &prop.value {
                volumes = Some(v.clone());
            }
        } else if prop.key == spa_sys::SPA_PROP_mute {
            if let Value::Bool(m) = prop.value {
                mute = Some(m);
            }
        }
    }
    if volumes.is_none() && mute.is_none() {
        return None;
    }
    Some((volumes.unwrap_or_default(), mute.unwrap_or(false)))
}

/// Parse an incoming standalone `Props` param pod (as delivered by `Node`'s `param` event).
pub fn parse_props(pod: &Pod) -> Option<(Vec<f32>, bool)> {
    let (_, value) = PodDeserializer::deserialize_from::<Value>(pod.as_bytes()).ok()?;
    let Value::Object(object) = value else {
        return None;
    };
    extract_volume_mute(&object.properties)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_volume_and_mute() {
        let bytes = build_props_pod(Some(&[0.5, 0.6]), Some(true));
        let pod = Pod::from_bytes(&bytes).expect("valid pod");
        let (volumes, mute) = parse_props(pod).expect("volume/mute present");
        assert_eq!(volumes, vec![0.5, 0.6]);
        assert!(mute);
    }

    #[test]
    fn round_trips_volume_only() {
        let bytes = build_props_pod(Some(&[1.0]), None);
        let pod = Pod::from_bytes(&bytes).expect("valid pod");
        let (volumes, mute) = parse_props(pod).expect("volume/mute present");
        assert_eq!(volumes, vec![1.0]);
        assert!(!mute);
    }

    #[test]
    fn empty_object_parses_to_none() {
        let bytes = build_props_pod(None, None);
        let pod = Pod::from_bytes(&bytes).expect("valid pod");
        assert!(parse_props(pod).is_none());
    }
}
