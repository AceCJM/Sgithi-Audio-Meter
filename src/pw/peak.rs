//! Per-node peak-level metering.
//!
//! Each watched node gets its own `pipewire::stream::Stream`, connected either directly to the
//! node (for a node that *produces* audio: a hardware mic or an app playback stream) or to that
//! node's monitor (for a node that *receives* audio: a hardware sink or an app recording stream -
//! see `model::is_sink_like`). The stream's `process` callback (run on this same PipeWire thread,
//! not a separate realtime thread - `Stream::connect` is called without `StreamFlags::RT_PROCESS`
//! - so it can safely share this thread with the rest of `PwState`) scans the raw F32LE samples
//! for the loudest one and throttles updates to the GTK thread to ~30Hz.

use std::time::{Duration, Instant};

use pipewire::core::CoreRc;
use pipewire::keys;
use pipewire::properties::properties;
use pipewire::spa;
use pipewire::spa::pod::Pod;
use pipewire::spa::utils::Direction;
use pipewire::stream::{StreamFlags, StreamListener, StreamRc};

use super::events::Event;

const THROTTLE: Duration = Duration::from_millis(33); // ~30Hz

/// `node.name` prefix every metering stream is created with. `registry.rs`'s Node discovery
/// filters out any node whose name starts with this - metering streams are themselves regular
/// PipeWire nodes (`media.class` `Stream/Input/Audio` or `Stream/Output/Audio`, assigned by
/// pipewire core from the `media.type`/`media.category` properties below), and without this
/// filter each one would be discovered as an application stream, get its own `Strip`, and that
/// `Strip` would start *another* metering stream on it - an exponential feedback loop that pegs a
/// CPU core within seconds (caught live: dozens of phantom "app streams" and 99% CPU usage).
pub const STREAM_NAME_PREFIX: &str = "sgithi-peak-";

/// Owns a single node's metering stream; dropping this tears it down.
pub struct PeakWatch {
    _stream: StreamRc,
    _listener: StreamListener<PeakUserData>,
}

struct PeakUserData {
    node_id: u32,
    event_tx: async_channel::Sender<Event>,
    last_sent: Instant,
}

/// Start metering `node_name`'s audio and send throttled `Event::PeakLevel`s for `node_id` until
/// the returned `PeakWatch` is dropped. Returns `None` (logging a warning) if the stream couldn't
/// be created or connected - metering is best-effort and shouldn't take down the app.
pub fn start(
    core: &CoreRc,
    event_tx: &async_channel::Sender<Event>,
    node_id: u32,
    node_name: &str,
    capture_sink: bool,
) -> Option<PeakWatch> {
    let stream_name = format!("{STREAM_NAME_PREFIX}{node_id}");
    let mut props = properties! {
        *keys::MEDIA_TYPE => "Audio",
        *keys::MEDIA_CATEGORY => "Monitor",
        *keys::MEDIA_ROLE => "Music",
        *keys::TARGET_OBJECT => node_name,
        // The `name` passed to `StreamRc::new` below does NOT become this stream's `node.name` -
        // that instead defaults to the whole process's name (`sgithi-audio-meter`, indistinguishable
        // from every other metering stream and, worse, unfilterable by `registry.rs`'s node
        // discovery). Setting it explicitly here is what `STREAM_NAME_PREFIX` filtering relies on.
        *keys::NODE_NAME => stream_name.clone(),
    };
    if capture_sink {
        props.insert(*keys::STREAM_CAPTURE_SINK, "true");
    }

    let stream = match StreamRc::new(core.clone(), &stream_name, props.into()) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("failed to create peak-meter stream for node {node_id}: {e}");
            return None;
        }
    };

    let user_data =
        PeakUserData { node_id, event_tx: event_tx.clone(), last_sent: Instant::now() - THROTTLE };
    let listener = stream
        .add_local_listener_with_user_data(user_data)
        .process(|stream, data| {
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let Some(chunk_data) = buffer.datas_mut().first_mut() else { return };
            let Some(samples) = chunk_data.data() else { return };

            // Samples are interleaved F32LE (requested via the EnumFormat param below); a single
            // aggregate peak across all channels is enough for this app's one-fader-per-node
            // display (see `NodeInfo::volume()`, which likewise averages rather than tracking
            // per-channel).
            let mut peak: f32 = 0.0;
            for raw in samples.chunks_exact(4) {
                let sample = f32::from_le_bytes(raw.try_into().unwrap());
                peak = peak.max(sample.abs());
            }

            let now = Instant::now();
            if now.duration_since(data.last_sent) >= THROTTLE {
                data.last_sent = now;
                let _ = data.event_tx.send_blocking(Event::PeakLevel { id: data.node_id, peak });
            }
        })
        .register();

    let listener = match listener {
        Ok(l) => l,
        Err(e) => {
            log::warn!("failed to register peak-meter listener for node {node_id}: {e}");
            return None;
        }
    };

    // A single `SPA_PARAM_EnumFormat` param requesting raw F32LE audio; channels/rate are left
    // unset to accept whatever the target node already uses.
    let mut audio_info = spa::param::audio::AudioInfoRaw::new();
    audio_info.set_format(spa::param::audio::AudioFormat::F32LE);
    let obj = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: audio_info.into(),
    };
    let Ok((cursor, _)) =
        spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &spa::pod::Value::Object(obj))
    else {
        log::warn!("failed to build format param for peak-meter stream on node {node_id}");
        return None;
    };
    let bytes = cursor.into_inner();
    let Some(format_pod) = Pod::from_bytes(&bytes) else {
        log::warn!("failed to parse format param for peak-meter stream on node {node_id}");
        return None;
    };

    if let Err(e) = stream.connect(
        Direction::Input,
        None,
        StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
        &mut [format_pod],
    ) {
        log::warn!("failed to connect peak-meter stream for node {node_id}: {e}");
        return None;
    }

    Some(PeakWatch { _stream: stream, _listener: listener })
}
