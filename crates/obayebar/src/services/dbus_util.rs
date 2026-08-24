//! Small D-Bus helpers shared by every service.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use futures_util::Stream;
use tokio::sync::Notify;

/// Panel-open signal: lets services throttle expensive detail refreshes to
/// the times when the UI is actually showing that detail.
#[derive(Debug)]
pub struct PanelSignal {
    open: AtomicBool,
    notify: OnceLock<Notify>,
}

impl PanelSignal {
    pub const fn new() -> Self {
        Self {
            open: AtomicBool::new(false),
            notify: OnceLock::new(),
        }
    }

    fn notify_cell(&self) -> &Notify {
        self.notify.get_or_init(Notify::new)
    }

    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Relaxed)
    }

    /// Called from the UI thread when the panel opens/closes.
    /// Wakes anyone waiting on `wait_change` so refreshes happen immediately.
    pub fn set(&self, open: bool) {
        let prev = self.open.swap(open, Ordering::Relaxed);
        if prev != open {
            self.notify_cell().notify_waiters();
        }
    }

    /// Wait for the next open/close transition.
    pub async fn changed(&self) {
        self.notify_cell().notified().await;
    }
}

/// One-shot "refresh now" signal from the UI to a polling service loop.
///
/// Unlike [`PanelSignal`], which is level-triggered (a waiter that misses the
/// wake-up still sees the new value via `is_open`), a refresh is purely an
/// edge — so a lost edge is a lost refresh. `notify_one` stores a permit, which
/// is what makes that safe: a request issued while the loop is *inside* its
/// body (a Secret Service round trip plus an HTTP fetch with a 15s timeout, in
/// the GitLab case) is still delivered when the loop comes back to wait.
///
/// `notify_waiters()` stores nothing, so an edge arriving while no waiter was
/// registered was simply dropped, with nothing logged.
#[derive(Debug)]
pub struct RefreshSignal {
    notify: OnceLock<Notify>,
}

impl RefreshSignal {
    pub const fn new() -> Self {
        Self {
            notify: OnceLock::new(),
        }
    }

    fn notify_cell(&self) -> &Notify {
        self.notify.get_or_init(Notify::new)
    }

    /// Ask the service loop to refresh at its next opportunity.
    pub fn request(&self) {
        self.notify_cell().notify_one();
    }

    /// Wait for a refresh request. Returns immediately if one is pending.
    pub async fn requested(&self) {
        self.notify_cell().notified().await;
    }
}

impl Default for RefreshSignal {
    fn default() -> Self {
        Self::new()
    }
}

/// Why a D-Bus proxy could not be built. Names the piece that was rejected,
/// which a bare `None` could not.
#[derive(Debug, thiserror::Error)]
#[error("proxy for {dest} {path} ({iface}): {source}")]
pub struct ProxyError {
    dest: String,
    path: String,
    iface: String,
    #[source]
    // Boxed: `zbus::Error` is large enough that an unboxed `Result` here
    // trips `clippy::result_large_err`.
    source: Box<zbus::Error>,
}

/// Build a `zbus::Proxy` from the four required pieces (connection, bus name,
/// object path, interface).
///
/// Returns `None` — every caller treats an unavailable service as "render it
/// as absent" rather than propagating — but the reason is *logged* first. This
/// is the single widest swallow site in the codebase: every service builds its
/// proxies here, and a construction failure used to disappear entirely,
/// leaving the module to report the service as off or empty with nothing to
/// distinguish that from a real absence.
pub async fn proxy<'a>(
    conn: &'a zbus::Connection,
    dest: &str,
    path: &str,
    iface: &str,
) -> Option<zbus::Proxy<'a>> {
    match try_proxy(conn, dest, path, iface).await {
        Ok(proxy) => Some(proxy),
        Err(e) => {
            log::warn!("dbus: {e}");
            None
        }
    }
}

/// The typed form of [`proxy`], for callers that want to report the failure
/// themselves.
pub async fn try_proxy<'a>(
    conn: &'a zbus::Connection,
    dest: &str,
    path: &str,
    iface: &str,
) -> Result<zbus::Proxy<'a>, ProxyError> {
    let describe = |source: zbus::Error| ProxyError {
        dest: dest.to_string(),
        path: path.to_string(),
        iface: iface.to_string(),
        source: Box::new(source),
    };
    zbus::proxy::Builder::new(conn)
        .destination(dest.to_string())
        .map_err(describe)?
        .path(path.to_string())
        .map_err(describe)?
        .interface(iface.to_string())
        .map_err(describe)?
        .build()
        .await
        .map_err(describe)
}

/// Build a `zbus::fdo::PropertiesProxy` for `(dest, path)`. Logs and returns
/// `None` on a construction error so optional services (e.g. `PowerProfiles`)
/// can fall through without the reason being lost.
pub async fn properties_proxy<'a>(
    conn: &'a zbus::Connection,
    dest: &str,
    path: &str,
) -> Option<zbus::fdo::PropertiesProxy<'a>> {
    let describe = |source: zbus::Error| ProxyError {
        dest: dest.to_string(),
        path: path.to_string(),
        iface: "org.freedesktop.DBus.Properties".to_string(),
        source: Box::new(source),
    };
    let built = zbus::fdo::PropertiesProxy::builder(conn)
        .destination(dest.to_string())
        .map_err(describe)
        .and_then(|builder| builder.path(path.to_string()).map_err(describe));
    match built {
        Ok(builder) => match builder.build().await {
            Ok(proxy) => Some(proxy),
            Err(e) => {
                log::warn!("dbus: {}", describe(e));
                None
            }
        },
        Err(e) => {
            log::warn!("dbus: {e}");
            None
        }
    }
}

/// Which system bus a stream should connect to.
#[derive(Debug, Clone, Copy)]
pub enum Bus {
    System,
    Session,
}

impl Bus {
    async fn connect(self) -> zbus::Result<zbus::Connection> {
        match self {
            Self::System => zbus::Connection::system().await,
            Self::Session => zbus::Connection::session().await,
        }
    }
}

/// Spawn the canonical "reconnect forever + run a signal loop" task shared by
/// all D-Bus-backed services. `run_loop` is invoked on every successful
/// connection; returning `Err(())` triggers a reconnect after `reconnect_delay`.
///
/// The returned stream yields every `T` sent through the channel by `run_loop`.
pub fn spawn_stream<T, F, Fut>(
    name: &'static str,
    bus: Bus,
    reconnect_delay: Duration,
    run_loop: F,
) -> impl Stream<Item = T>
where
    T: Send + 'static,
    F: Fn(zbus::Connection, tokio::sync::mpsc::UnboundedSender<T>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), ()>> + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    tokio::spawn(async move {
        loop {
            if tx.is_closed() {
                return;
            }
            let conn = loop {
                if let Ok(c) = bus.connect().await {
                    break c;
                }
                if tx.is_closed() {
                    return;
                }
                log::warn!("{name}: failed to connect to D-Bus, retrying");
                tokio::time::sleep(reconnect_delay).await;
            };

            if run_loop(conn, tx.clone()).await.is_err() {
                if tx.is_closed() {
                    return;
                }
                log::warn!("{name}: signal loop ended, reconnecting");
                tokio::time::sleep(reconnect_delay).await;
            }
        }
    });

    tokio_stream::wrappers::UnboundedReceiverStream::new(rx)
}

/// Send `new` through `tx` only if it differs from `last`. On a successful
/// send `last` is updated. Returns `Err(())` when the channel is closed.
pub fn send_if_changed<T: Clone + PartialEq>(
    tx: &tokio::sync::mpsc::UnboundedSender<T>,
    last: &mut T,
    new: T,
) -> Result<(), ()> {
    if new == *last {
        return Ok(());
    }
    *last = new.clone();
    tx.send(new).map_err(|_| ())
}
