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
        pod::{deserialize::PodDeserializer, Value},
    },
    types::ObjectType,
};

#[derive(Debug, Clone)]
pub struct AudioInfo {
    pub volume: f32,
    pub muted: bool,
    pub icon_name: &'static str,
}

impl Default for AudioInfo {
    fn default() -> Self {
        Self {
            volume: 0.0,
            muted: false,
            icon_name: crate::style::ICON_VOLUME_OFF,
        }
    }
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
fn parse_props_pod(pod_bytes: &[u8]) -> Option<(f32, bool)> {
    let (_, value) = PodDeserializer::deserialize_from::<Value>(pod_bytes).ok()?;
    let Value::Object(object) = value else {
        return None;
    };

    let mut volume: Option<f32> = None;
    let mut muted: Option<bool> = None;

    for prop in &object.properties {
        match prop.key {
            spa::sys::SPA_PROP_channelVolumes => {
                if let Value::ValueArray(spa::pod::ValueArray::Float(ref vols)) = prop.value {
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

    Some((volume.unwrap_or(0.0), muted.unwrap_or(false)))
}

struct Proxies {
    proxies: HashMap<u32, Box<dyn ProxyT>>,
    listeners: HashMap<u32, Vec<Box<dyn Listener>>>,
}

impl Proxies {
    fn new() -> Self {
        Self {
            proxies: HashMap::new(),
            listeners: HashMap::new(),
        }
    }

    fn add(&mut self, id: u32, proxy: Box<dyn ProxyT>, listener: Box<dyn Listener>) {
        self.proxies.insert(id, proxy);
        self.listeners.entry(id).or_default().push(listener);
    }

    fn add_listener(&mut self, id: u32, listener: Box<dyn Listener>) {
        self.listeners.entry(id).or_default().push(listener);
    }

    fn remove(&mut self, id: u32) {
        self.proxies.remove(&id);
        self.listeners.remove(&id);
    }
}

/// Bind to a sink node, subscribe to Props, and forward volume updates.
fn bind_sink_node(
    node: Node,
    tx: &tokio::sync::mpsc::UnboundedSender<AudioInfo>,
    proxies: &Rc<RefCell<Proxies>>,
) {
    node.subscribe_params(&[spa::param::ParamType::Props]);

    let tx2 = tx.clone();
    let obj_listener = node
        .add_listener_local()
        .param(move |_seq, id, _index, _next, param| {
            if id != spa::param::ParamType::Props {
                return;
            }
            let Some(pod) = param else { return };
            let Some((volume, muted)) = parse_props_pod(pod.as_bytes()) else {
                return;
            };
            let _ = tx2.send(AudioInfo {
                volume,
                muted,
                icon_name: volume_icon(volume, muted),
            });
        })
        .register();

    let proxy_id = node.upcast_ref().id();
    let proxies_weak = Rc::downgrade(proxies);
    let proxy_listener = node
        .upcast_ref()
        .add_listener_local()
        .removed(move || {
            if let Some(p) = proxies_weak.upgrade() {
                p.borrow_mut().remove(proxy_id);
            }
        })
        .register();

    let mut p = proxies.borrow_mut();
    p.add(proxy_id, Box::new(node), Box::new(obj_listener));
    p.add_listener(proxy_id, Box::new(proxy_listener));
}

/// Bind to a metadata object and watch for default sink changes.
fn bind_metadata(
    metadata: Metadata,
    default_sink_serial: &Rc<RefCell<Option<String>>>,
    ml_weak: &Rc<pw::main_loop::WeakMainLoop>,
    proxies: &Rc<RefCell<Proxies>>,
) {
    let default_sink_serial2 = Rc::clone(default_sink_serial);
    let ml_weak2 = Rc::clone(ml_weak);
    let obj_listener = metadata
        .add_listener_local()
        .property(move |_subject, key, _type_, value| {
            if key != Some("default.audio.sink") {
                return 0;
            }
            // value is JSON like {"name":"..."}; extract the name
            let new_name = value
                .and_then(|v| serde_json::from_str::<serde_json::Value>(v).ok())
                .and_then(|j| j.get("name").and_then(|n| n.as_str()).map(String::from));
            *default_sink_serial2.borrow_mut() = new_name;
            // Restart the loop to re-bind to the new default sink
            if let Some(ml) = ml_weak2.upgrade() {
                ml.quit();
            }
            0
        })
        .register();

    let proxy_id = metadata.upcast_ref().id();
    let proxies_weak = Rc::downgrade(proxies);
    let proxy_listener = metadata
        .upcast_ref()
        .add_listener_local()
        .removed(move || {
            if let Some(p) = proxies_weak.upgrade() {
                p.borrow_mut().remove(proxy_id);
            }
        })
        .register();

    let mut p = proxies.borrow_mut();
    p.add(proxy_id, Box::new(metadata), Box::new(obj_listener));
    p.add_listener(proxy_id, Box::new(proxy_listener));
}

fn run_pipewire_monitor(tx: tokio::sync::mpsc::UnboundedSender<AudioInfo>) {
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

    let proxies = Rc::new(RefCell::new(Proxies::new()));
    let node_serials: Rc<RefCell<HashMap<u32, String>>> = Rc::new(RefCell::new(HashMap::new()));
    let default_sink_serial: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

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

    let ml_weak_for_metadata = Rc::new(main_loop.downgrade());
    let _registry_listener = registry
        .add_listener_local()
        .global({
            let proxies = Rc::clone(&proxies);
            let node_serials = Rc::clone(&node_serials);
            let default_sink_serial = Rc::clone(&default_sink_serial);
            let ml_weak_for_metadata = Rc::clone(&ml_weak_for_metadata);

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
                        node_serials.borrow_mut().insert(obj.id, serial.clone());

                        let is_default = default_sink_serial
                            .borrow()
                            .as_deref()
                            .is_some_and(|s| s == serial);

                        if !is_default {
                            return;
                        }

                        let Ok(node): Result<Node, _> = registry.bind(obj) else {
                            return;
                        };

                        bind_sink_node(node, &tx, &proxies);
                    }
                    ObjectType::Metadata => {
                        let Ok(metadata): Result<Metadata, _> = registry.bind(obj) else {
                            return;
                        };

                        bind_metadata(
                            metadata,
                            &default_sink_serial,
                            &ml_weak_for_metadata,
                            &proxies,
                        );
                    }
                    _ => {}
                }
            }
        })
        .global_remove({
            let proxies = Rc::clone(&proxies);
            let node_serials = Rc::clone(&node_serials);
            move |id| {
                proxies.borrow_mut().remove(id);
                node_serials.borrow_mut().remove(&id);
            }
        })
        .register();

    loop {
        main_loop.run();
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

pub fn stream() -> impl Stream<Item = AudioInfo> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<AudioInfo>();

    std::thread::spawn(move || run_pipewire_monitor(tx));

    futures_util::stream::unfold(rx, |mut rx| async {
        let info = rx.recv().await?;
        Some((info, rx))
    })
}
