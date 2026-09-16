//! A `Device`'s active `Profile` picks which operating mode the whole card is in (e.g. "Off",
//! "Analog Stereo Duplex", "Pro Audio" for an audio interface) - distinct from a `Route`
//! (device_route.rs), which is per-port volume/mute within whatever profile is currently active.

use pipewire::spa::param::ParamType;
use pipewire::spa::pod::{deserialize::PodDeserializer, Pod, Property, Value};
use pipewire::spa::sys as spa_sys;
use pipewire::spa::utils::SpaTypes;

use crate::model::ProfileOption;

use super::node_props;

/// Parse one `EnumProfile` result (one call per available profile) or a `Profile` result (the
/// currently active one) - both param types share the same pod shape.
pub fn parse_profile(pod: &Pod) -> Option<ProfileOption> {
    let (_, value) = PodDeserializer::deserialize_from::<Value>(pod.as_bytes()).ok()?;
    let Value::Object(object) = value else {
        return None;
    };

    let mut index = None;
    let mut name = None;
    let mut description = None;

    for prop in &object.properties {
        if prop.key == spa_sys::SPA_PARAM_PROFILE_index {
            if let Value::Int(i) = prop.value {
                index = Some(i);
            }
        } else if prop.key == spa_sys::SPA_PARAM_PROFILE_name {
            if let Value::String(s) = &prop.value {
                name = Some(s.clone());
            }
        } else if prop.key == spa_sys::SPA_PARAM_PROFILE_description {
            if let Value::String(s) = &prop.value {
                description = Some(s.clone());
            }
        }
    }

    Some(ProfileOption { index: index?, description: description.or(name)? })
}

/// Build a `Profile` pod selecting the given profile as active, for `Device::set_param`.
pub fn build_set_profile_pod(index: i32) -> Vec<u8> {
    let value = Value::Object(pipewire::spa::pod::Object {
        type_: SpaTypes::ObjectParamProfile.as_raw(),
        id: ParamType::Profile.as_raw(),
        properties: vec![
            Property::new(spa_sys::SPA_PARAM_PROFILE_index, Value::Int(index)),
            Property::new(spa_sys::SPA_PARAM_PROFILE_save, Value::Bool(true)),
        ],
    });
    node_props::serialize(&value)
}
