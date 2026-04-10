use futures_util::Stream;
use std::rc::Rc;
use std::{cell::RefCell, collections::HashMap};

use pipewire as pw;
use pw::{
    metadata::Metadata,
    node::Node,
    proxy::{Listener, ProxyT},
    spa::{
        self,
        pod::{
            deserialize::PodDeserializer, serialize::PodSerializer, Pod, Property, PropertyFlags,
            Value, ValueArray,
        },
        utils::SpaTypes,
    },
    types::ObjectType,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkInfo {
    pub id: u32,
    pub serial: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct AudioInfo {
    pub volume: f32,
    pub muted: bool,
    pub icon_name: &'static str,
    pub sinks: Vec<SinkInfo>,
    pub default_sink_name: Option<String>,
}

impl Default for AudioInfo {
    fn default() -> Self {
        Self {
            volume: 0.0,
            muted: false,
            icon_name: crate::style::ICON_VOLUME_OFF,
            sinks: Vec::new(),
            default_sink_name: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum AudioCommand {
    Volume(f32),
    Mute(bool),
    DefaultSink { id: u32 },
}

fn volume_icon(volume: f32, muted: bool) -> &'static str {
    if muted {
        return crate::style::ICON_VOLUME_OFF;
    }
    let pct = volume * 100.0;
    if pct >= 66.0 {
        crate::style::ICON_VOLUME_UP
    } else if pct >= 33.0 {
        crate::style::ICON_VOLUME_DOWN
    } else if pct >= 1.0 {
        crate::style::ICON_VOLUME_MUTE
    } else {
        crate::style::ICON_VOLUME_OFF
    }
}

/// Parse volume and mute from a Props pod.
fn parse_props_pod(pod_bytes: &[u8]) -> Option<(f32, bool, usize)> {
    let (_, value) = PodDeserializer::deserialize_from::<Value>(pod_bytes).ok()?;
    let Value::Object(object) = value else {
        return None;
    };

    let mut volume: Option<f32> = None;
    let mut muted: Option<bool> = None;
    let mut channels: usize = 2;

    for prop in &object.properties {
        match prop.key {
            spa::sys::SPA_PROP_channelVolumes => {
                if let Value::ValueArray(ValueArray::Float(ref vols)) = prop.value {
                    channels = vols.len();
                    if let Some(&v) = vols.first() {
                        // PipeWire uses cubic volume; convert to linear percentage
                        volume = Some(v.cbrt());
                    }
                }
            }
            spa::sys::SPA_PROP_volume if volume.is_none() => {
                if let Value::Float(v) = prop.value {
                    volume = Some(v.cbrt());
                }
            }
            spa::sys::SPA_PROP_mute => {
                if let Value::Bool(m) = prop.value {
                    muted = Some(m);
                }
            }
            _ => {}
        }
    }

    Some((volume.unwrap_or(0.0), muted.unwrap_or(false), channels))
}

/// Build a Props pod to set channel volumes on a node.
fn build_volume_pod(linear_volume: f32, channels: usize) -> Option<Vec<u8>> {
    let cubic = linear_volume.powi(3);
    let volumes = vec![cubic; channels.max(2)];

    let object = spa::pod::Object {
        type_: SpaTypes::ObjectParamProps.as_raw(),
        id: spa::param::ParamType::Props.as_raw(),
        properties: vec![Property {
            key: spa::sys::SPA_PROP_channelVolumes,
            flags: PropertyFlags::empty(),
            value: Value::ValueArray(ValueArray::Float(volumes)),
        }],
    };

    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object))
        .ok()
        .map(|s| s.0.into_inner())
}

/// Build a Props pod to set mute on a node.
fn build_mute_pod(muted: bool) -> Option<Vec<u8>> {
    let object = spa::pod::Object {
        type_: SpaTypes::ObjectParamProps.as_raw(),
        id: spa::param::ParamType::Props.as_raw(),
        properties: vec![Property {
            key: spa::sys::SPA_PROP_mute,
            flags: PropertyFlags::empty(),
            value: Value::Bool(muted),
        }],
    };

    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object))
        .ok()
        .map(|s| s.0.into_inner())
}

/// Shared state for the `PipeWire` monitor thread.
struct PwState {
    /// Per-sink volume/mute/channels, keyed by node name
    sink_volumes: HashMap<String, (f32, bool, usize)>,
    sinks: Vec<SinkInfo>,
    default_sink_name: Option<String>,
}

impl PwState {
    fn new() -> Self {
        Self {
            sink_volumes: HashMap::new(),
            sinks: Vec::new(),
            default_sink_name: None,
        }
    }

    fn default_volume(&self) -> (f32, bool) {
        self.default_sink_name
            .as_deref()
            .and_then(|name| self.sink_volumes.get(name))
            .map_or((0.0, false), |&(vol, muted, _)| (vol, muted))
    }

    fn default_channels(&self) -> usize {
        self.default_sink_name
            .as_deref()
            .and_then(|name| self.sink_volumes.get(name))
            .map_or(2, |&(_, _, ch)| ch)
    }

    fn to_audio_info(&self) -> AudioInfo {
        let (volume, muted) = self.default_volume();
        AudioInfo {
            volume,
            muted,
            icon_name: volume_icon(volume, muted),
            sinks: self.sinks.clone(),
            default_sink_name: self.default_sink_name.clone(),
        }
    }
}

/// Typed proxy storage for command access.
struct PwProxies {
    /// Sink nodes keyed by node name, for `set_param`
    sink_nodes: HashMap<String, Node>,
    /// Metadata proxy for `set_property`
    metadata: Option<Metadata>,
    /// All listeners (kept alive)
    listeners: Vec<Box<dyn Listener>>,
}

impl PwProxies {
    fn new() -> Self {
        Self {
            sink_nodes: HashMap::new(),
            metadata: None,
            listeners: Vec::new(),
        }
    }

    fn remove_sink(&mut self, name: &str) {
        self.sink_nodes.remove(name);
    }
}

/// Bind to a sink node, subscribe to Props, and store volume updates.
fn bind_sink_node(
    node: Node,
    node_name: String,
    tx: &tokio::sync::mpsc::UnboundedSender<AudioInfo>,
    proxies: &Rc<RefCell<PwProxies>>,
    state: &Rc<RefCell<PwState>>,
) {
    node.subscribe_params(&[spa::param::ParamType::Props]);

    let tx2 = tx.clone();
    let state2 = Rc::clone(state);
    let name_for_remove = node_name.clone();
    let name_for_insert = node_name.clone();
    let obj_listener = node
        .add_listener_local()
        .param(move |_seq, id, index, _next, param| {
            // Props are enumerated across multiple indices: index 0 has the
            // primary hardware props (channelVolumes, mute), higher indices
            // hold software/fallback properties.  We only care about index 0.
            if id != spa::param::ParamType::Props || index != 0 {
                return;
            }
            let Some(pod) = param else { return };
            let Some((volume, muted, channels)) = parse_props_pod(pod.as_bytes()) else {
                return;
            };
            let mut s = state2.borrow_mut();
            s.sink_volumes
                .insert(node_name.clone(), (volume, muted, channels));
            let _ = tx2.send(s.to_audio_info());
        })
        .register();
    let proxies_weak = Rc::downgrade(proxies);
    let proxy_listener = node
        .upcast_ref()
        .add_listener_local()
        .removed(move || {
            if let Some(p) = proxies_weak.upgrade() {
                p.borrow_mut().remove_sink(&name_for_remove);
            }
        })
        .register();

    let mut p = proxies.borrow_mut();
    p.listeners.push(Box::new(obj_listener));
    p.listeners.push(Box::new(proxy_listener));
    p.sink_nodes.insert(name_for_insert, node);
}

/// Bind to a metadata object and watch for default sink changes.
fn bind_metadata(
    metadata: Metadata,
    state: &Rc<RefCell<PwState>>,
    tx: &tokio::sync::mpsc::UnboundedSender<AudioInfo>,
    proxies: &Rc<RefCell<PwProxies>>,
) {
    let state2 = Rc::clone(state);
    let tx2 = tx.clone();
    let proxies2 = Rc::clone(proxies);
    let obj_listener = metadata
        .add_listener_local()
        .property(move |_subject, key, _type_, value| {
            if key != Some("default.audio.sink") {
                return 0;
            }
            let new_name = value
                .and_then(|v| serde_json::from_str::<serde_json::Value>(v).ok())
                .and_then(|j| j.get("name").and_then(|n| n.as_str()).map(String::from));
            let mut s = state2.borrow_mut();
            s.default_sink_name.clone_from(&new_name);
            let _ = tx2.send(s.to_audio_info());
            drop(s);
            // Re-subscribe the new default sink's params so we get a fresh
            // volume update now that default_sink_name is set
            if let Some(name) = &new_name {
                let p = proxies2.borrow();
                if let Some(node) = p.sink_nodes.get(name) {
                    node.subscribe_params(&[spa::param::ParamType::Props]);
                }
            }
            0
        })
        .register();

    let proxies_weak = Rc::downgrade(proxies);
    let proxy_listener = metadata
        .upcast_ref()
        .add_listener_local()
        .removed(move || {
            if let Some(p) = proxies_weak.upgrade() {
                p.borrow_mut().metadata = None;
            }
        })
        .register();

    let mut p = proxies.borrow_mut();
    p.listeners.push(Box::new(obj_listener));
    p.listeners.push(Box::new(proxy_listener));
    p.metadata = Some(metadata);
}

/// Process a command using native `PipeWire` API.
fn process_command(cmd: &AudioCommand, proxies: &PwProxies, state: &PwState) {
    match *cmd {
        AudioCommand::Volume(vol) => {
            let Some(default_name) = state.default_sink_name.as_deref() else {
                return;
            };
            let Some(node) = proxies.sink_nodes.get(default_name) else {
                return;
            };
            let channels = state.default_channels();
            if let Some(bytes) = build_volume_pod(vol, channels) {
                if let Some(pod) = Pod::from_bytes(&bytes) {
                    node.set_param(spa::param::ParamType::Props, 0, pod);
                }
            }
        }
        AudioCommand::Mute(muted) => {
            let Some(default_name) = state.default_sink_name.as_deref() else {
                return;
            };
            let Some(node) = proxies.sink_nodes.get(default_name) else {
                return;
            };
            if let Some(bytes) = build_mute_pod(muted) {
                if let Some(pod) = Pod::from_bytes(&bytes) {
                    node.set_param(spa::param::ParamType::Props, 0, pod);
                }
            }
        }
        AudioCommand::DefaultSink { id } => {
            let Some(metadata) = proxies.metadata.as_ref() else {
                return;
            };
            // Find the node name for this id
            let Some(sink) = state.sinks.iter().find(|s| s.id == id) else {
                return;
            };
            let json = format!(r#"{{"name":"{}"}}"#, sink.name);
            metadata.set_property(
                0,
                "default.audio.sink",
                Some("Spa:String:JSON"),
                Some(&json),
            );
        }
    }
}

#[allow(clippy::too_many_lines, clippy::needless_pass_by_value)]
fn run_pipewire_monitor(
    tx: tokio::sync::mpsc::UnboundedSender<AudioInfo>,
    cmd_rx: std::sync::mpsc::Receiver<AudioCommand>,
) {
    pw::init();

    let Ok(main_loop) = pw::main_loop::MainLoop::new(None) else {
        log::error!("Failed to create PipeWire main loop");
        return;
    };
    let Ok(context) = pw::context::Context::new(&main_loop) else {
        log::error!("Failed to create PipeWire context");
        return;
    };
    let Ok(core) = context.connect(None) else {
        log::error!("Failed to connect to PipeWire");
        return;
    };

    let Ok(registry) = core.get_registry() else {
        log::error!("Failed to get PipeWire registry");
        return;
    };
    let registry = Rc::new(registry);
    let registry_weak = Rc::downgrade(&registry);

    let proxies = Rc::new(RefCell::new(PwProxies::new()));
    let state = Rc::new(RefCell::new(PwState::new()));

    let main_loop_weak = main_loop.downgrade();
    let _core_listener = core
        .add_listener_local()
        .error(move |id, _seq, _res, message| {
            log::error!("PipeWire core error id={id}: {message}");
            if id == 0 {
                if let Some(ml) = main_loop_weak.upgrade() {
                    ml.quit();
                }
            }
        })
        .register();

    // Timer to poll command channel
    let timer_proxies = Rc::clone(&proxies);
    let timer_state = Rc::clone(&state);
    let cmd_timer = main_loop.loop_().add_timer(move |_| {
        while let Ok(cmd) = cmd_rx.try_recv() {
            let p = timer_proxies.borrow();
            let s = timer_state.borrow();
            process_command(&cmd, &p, &s);
        }
    });
    cmd_timer.update_timer(
        Some(std::time::Duration::from_millis(16)),
        Some(std::time::Duration::from_millis(16)),
    );

    let _registry_listener = registry
        .add_listener_local()
        .global({
            let proxies = Rc::clone(&proxies);
            let state = Rc::clone(&state);
            let tx = tx.clone();

            move |obj| {
                let Some(registry) = registry_weak.upgrade() else {
                    return;
                };

                match obj.type_ {
                    ObjectType::Node => {
                        let Some(props) = obj.props.as_ref() else {
                            return;
                        };

                        if props.get("media.class").unwrap_or_default() != "Audio/Sink" {
                            return;
                        }

                        let serial = props.get("object.serial").unwrap_or_default().to_string();
                        let name = props.get("node.name").unwrap_or("unknown").to_string();
                        let description =
                            props.get("node.description").unwrap_or(&name).to_string();

                        {
                            let mut s = state.borrow_mut();
                            s.sinks.retain(|sink| sink.id != obj.id);
                            s.sinks.push(SinkInfo {
                                id: obj.id,
                                serial,
                                name: name.clone(),
                                description,
                            });
                            let _ = tx.send(s.to_audio_info());
                        }

                        let Ok(node): Result<Node, _> = registry.bind(obj) else {
                            return;
                        };

                        bind_sink_node(node, name, &tx, &proxies, &state);
                    }
                    ObjectType::Metadata => {
                        let Some(props) = obj.props.as_ref() else {
                            return;
                        };
                        if props.get("metadata.name").unwrap_or_default() != "default" {
                            return;
                        }
                        let Ok(metadata): Result<Metadata, _> = registry.bind(obj) else {
                            return;
                        };

                        bind_metadata(metadata, &state, &tx, &proxies);
                    }
                    _ => {}
                }
            }
        })
        .global_remove({
            let proxies = Rc::clone(&proxies);
            let state = Rc::clone(&state);
            move |id| {
                let mut s = state.borrow_mut();
                if let Some(pos) = s.sinks.iter().position(|sink| sink.id == id) {
                    let name = s.sinks.remove(pos).name;
                    s.sink_volumes.remove(&name);
                    proxies.borrow_mut().remove_sink(&name);
                    let _ = tx.send(s.to_audio_info());
                }
            }
        })
        .register();

    loop {
        main_loop.run();
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

static CMD_TX: std::sync::OnceLock<std::sync::mpsc::Sender<AudioCommand>> =
    std::sync::OnceLock::new();

/// Send a command to the `PipeWire` thread.
pub fn send_command(cmd: AudioCommand) {
    if let Some(tx) = CMD_TX.get() {
        let _ = tx.send(cmd);
    }
}

pub fn stream() -> impl Stream<Item = AudioInfo> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<AudioInfo>();
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<AudioCommand>();
    let _ = CMD_TX.set(cmd_tx);

    std::thread::spawn(move || run_pipewire_monitor(tx, cmd_rx));

    futures_util::stream::unfold(rx, |mut rx| async {
        let info = rx.recv().await?;
        Some((info, rx))
    })
}
