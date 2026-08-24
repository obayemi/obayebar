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

#[derive(Debug, Clone, PartialEq)]
pub struct AudioInfo {
    pub volume: f32,
    pub muted: bool,
    pub icon_name: &'static str,
    pub sinks: Vec<SinkInfo>,
    pub default_sink_name: Option<String>,
    /// Whether a live `PipeWire` connection is backing these values. False
    /// means the reading is not live, so the UI must not present it as the
    /// current volume — the previous code left a stale reading on screen while
    /// every slider drag was silently dropped.
    pub available: bool,
}

impl Default for AudioInfo {
    fn default() -> Self {
        Self {
            volume: 0.0,
            muted: false,
            icon_name: obayebar::style::ICON_VOLUME_OFF,
            sinks: Vec::new(),
            default_sink_name: None,
            available: false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum AudioCommand {
    Volume(f32),
    Mute(bool),
    DefaultSink { id: u32 },
}

pub fn volume_icon(volume: f32, muted: bool) -> &'static str {
    if muted {
        return obayebar::style::ICON_VOLUME_OFF;
    }
    let pct = volume * 100.0;
    if pct >= 66.0 {
        obayebar::style::ICON_VOLUME_UP
    } else if pct >= 33.0 {
        obayebar::style::ICON_VOLUME_DOWN
    } else if pct >= 1.0 {
        obayebar::style::ICON_VOLUME_MUTE
    } else {
        obayebar::style::ICON_VOLUME_OFF
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
    last_sent: Option<AudioInfo>,
}

impl PwState {
    fn new() -> Self {
        Self {
            sink_volumes: HashMap::new(),
            sinks: Vec::new(),
            default_sink_name: None,
            last_sent: None,
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
            // Reached only from a live registry callback.
            available: true,
        }
    }

    /// Build audio info and send only if it differs from last sent value.
    fn send_if_changed(&mut self, tx: &tokio::sync::mpsc::UnboundedSender<AudioInfo>) {
        let info = self.to_audio_info();
        if self.last_sent.as_ref() != Some(&info) {
            self.last_sent = Some(info.clone());
            let _ = tx.send(info);
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
            s.send_if_changed(&tx2);
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
            s.send_if_changed(&tx2);
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

/// Delay between `PipeWire` connection attempts.
const PW_RECONNECT_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

/// Own the `PipeWire` monitor thread: connect, run until the connection dies,
/// publish an unavailable state, wait, and try again.
///
/// Every failure used to `return`, ending the thread and dropping `tx`; iced
/// never restarts a finished subscription recipe, so the volume widget stayed
/// at `AudioInfo::default()` (0%, muted icon) for the rest of the session after
/// a single early failure — the common case being the bar starting from
/// `exec-once` before the pipewire user unit is up. The steady state was no
/// better: a core error called `main_loop.quit()` and the old trailing
/// `loop { main_loop.run(); sleep(100ms) }` re-entered with a dead core and a
/// `PwState` that was never reset, so stale sinks kept rendering and commands
/// were written to dead node proxies.
#[allow(clippy::needless_pass_by_value)]
fn run_pipewire_monitor(
    tx: tokio::sync::mpsc::UnboundedSender<AudioInfo>,
    cmd_rx: std::sync::mpsc::Receiver<AudioCommand>,
) {
    pw::init();

    // The receiver has to outlive every attempt: `add_timer` wants an
    // `Fn(u64) + 'static` closure so it cannot borrow a per-attempt local, and
    // re-creating the channel would strand the `CMD_TX` `OnceLock` that
    // `send_command` publishes through. One long-lived channel, shared by `Rc`
    // into each attempt's timer.
    let cmd_rx = Rc::new(cmd_rx);

    while !tx.is_closed() {
        match try_run_pipewire(&tx, &cmd_rx) {
            Ok(()) => log::warn!("audio: PipeWire connection ended, reconnecting"),
            Err(reason) => log::warn!("audio: PipeWire unavailable ({reason}), retrying"),
        }
        if tx.is_closed() {
            return;
        }
        // Tell the UI its reading is no longer live rather than leaving a
        // stale volume that every drag silently fails to change.
        let _ = tx.send(AudioInfo::default());
        std::thread::sleep(PW_RECONNECT_DELAY);
    }
}

/// One `PipeWire` connection attempt, with fresh state and proxies.
///
/// Returns `Ok` when the main loop exited (the connection died and we should
/// reconnect) and `Err` with a reason when setup never got that far.
#[allow(clippy::too_many_lines)]
fn try_run_pipewire(
    tx: &tokio::sync::mpsc::UnboundedSender<AudioInfo>,
    cmd_rx: &Rc<std::sync::mpsc::Receiver<AudioCommand>>,
) -> Result<(), &'static str> {
    let main_loop = pw::main_loop::MainLoopRc::new(None).map_err(|_| "cannot create main loop")?;
    let context =
        pw::context::ContextRc::new(&main_loop, None).map_err(|_| "cannot create context")?;
    let core = context.connect_rc(None).map_err(|_| "cannot connect")?;
    let registry = core.get_registry_rc().map_err(|_| "cannot get registry")?;
    let registry_weak = registry.downgrade();
    let tx = tx.clone();
    let cmd_rx = Rc::clone(cmd_rx);

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
                            s.send_if_changed(&tx);
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
                    s.send_if_changed(&tx);
                }
            }
        })
        .register();

    // Runs until the core-error listener calls `quit()`. Everything above is
    // then dropped, so the next attempt starts from clean state instead of
    // re-entering with a dead core.
    main_loop.run();
    Ok(())
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
