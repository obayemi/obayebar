//! xdg-activation tokens, so an app the bar hands a click to can take focus.
//!
//! The compositor only grants a token for the serial of a recent input event
//! delivered to the requesting client. The bar therefore owns its Wayland
//! connection and shares it with iced: this module binds its own seat and
//! pointer on that same client, remembers the serial of the latest click, and
//! trades it for a token on demand. Whether the token then moves focus is the
//! compositor's call (`misc:focus_on_activate` on Hyprland).

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::oneshot;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_token_v1::{
    self, XdgActivationTokenV1,
};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;

/// How long to wait for the compositor to answer a token request before
/// handing the click over without one.
const TOKEN_TIMEOUT: Duration = Duration::from_millis(300);

struct Activation {
    connection: Connection,
    queue: QueueHandle<Tracker>,
    seat: WlSeat,
    manager: XdgActivationV1,
    last_click: Arc<Mutex<Option<u32>>>,
}

static ACTIVATION: OnceLock<Activation> = OnceLock::new();

/// Dispatch state of the activation queue: the serial of the latest click,
/// shared with [`Activation`].
struct Tracker {
    last_click: Arc<Mutex<Option<u32>>>,
}

type PendingToken = Mutex<Option<oneshot::Sender<String>>>;

/// Open the bar's Wayland connection and start tracking clicks on it.
///
/// Returns the connection for iced to share, or `None` when it cannot be
/// opened, in which case iced opens its own and no token is ever granted.
pub fn connect() -> Option<Connection> {
    let connection = Connection::connect_to_env()
        .inspect_err(|e| log::warn!("activation: no Wayland connection: {e}"))
        .ok()?;
    if let Err(e) = track_clicks(&connection) {
        log::warn!("activation: tokens unavailable: {e}");
    }
    Some(connection)
}

fn track_clicks(connection: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    let (globals, mut queue) = registry_queue_init::<Tracker>(connection)?;
    let qh = queue.handle();
    let mut tracker = Tracker {
        last_click: Arc::default(),
    };
    let activation = Activation {
        connection: connection.clone(),
        seat: globals.bind(&qh, 1..=5, ())?,
        manager: globals.bind(&qh, 1..=1, ())?,
        queue: qh,
        last_click: Arc::clone(&tracker.last_click),
    };
    ACTIVATION
        .set(activation)
        .map_err(|_| "already tracking clicks")?;
    std::thread::Builder::new()
        .name("activation".into())
        .spawn(move || loop {
            if let Err(e) = queue.blocking_dispatch(&mut tracker) {
                log::warn!("activation: event queue stopped: {e}");
                break;
            }
        })?;
    Ok(())
}

/// Ask the compositor for a token backed by the latest click on the bar.
///
/// `None` when no click was seen yet, the compositor does not support
/// xdg-activation, or it did not answer in time.
pub async fn token() -> Option<String> {
    let activation = ACTIVATION.get()?;
    let serial = (*activation.last_click.lock().ok()?)?;
    let (sender, receiver) = oneshot::channel();
    let request = activation
        .manager
        .get_activation_token(&activation.queue, Mutex::new(Some(sender)));
    request.set_serial(serial, &activation.seat);
    request.commit();
    activation
        .connection
        .flush()
        .inspect_err(|e| log::warn!("activation: token request not sent: {e}"))
        .ok()?;
    tokio::time::timeout(TOKEN_TIMEOUT, receiver)
        .await
        .inspect_err(|_| log::warn!("activation: no token granted for serial {serial}"))
        .ok()?
        .ok()
}

impl Dispatch<WlRegistry, GlobalListContents> for Tracker {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as wayland_client::Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for Tracker {
    fn event(
        _: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        (): &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        {
            if capabilities.contains(wl_seat::Capability::Pointer) {
                seat.get_pointer(qh, ());
            }
        }
    }
}

impl Dispatch<WlPointer, ()> for Tracker {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_pointer::Event::Button { serial, .. } = event {
            if let Ok(mut last_click) = state.last_click.lock() {
                *last_click = Some(serial);
            }
        }
    }
}

impl Dispatch<XdgActivationTokenV1, PendingToken> for Tracker {
    fn event(
        _: &mut Self,
        proxy: &XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        pending: &PendingToken,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token } = event {
            if let Some(sender) = pending.lock().ok().and_then(|mut s| s.take()) {
                let _ = sender.send(token);
            }
            proxy.destroy();
        }
    }
}

delegate_noop!(Tracker: ignore XdgActivationV1);
