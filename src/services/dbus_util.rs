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

/// Build a `zbus::Proxy` from the four required pieces (connection, bus name,
/// object path, interface). Returns `None` on any construction error.
pub async fn proxy<'a>(
    conn: &'a zbus::Connection,
    dest: &str,
    path: &str,
    iface: &str,
) -> Option<zbus::Proxy<'a>> {
    zbus::proxy::Builder::new(conn)
        .destination(dest.to_string())
        .ok()?
        .path(path.to_string())
        .ok()?
        .interface(iface.to_string())
        .ok()?
        .build()
        .await
        .ok()
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
