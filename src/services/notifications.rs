use futures_util::Stream;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use zbus::object_server::SignalEmitter;

#[derive(Debug, Clone)]
pub struct NotificationData {
    pub id: u32,
    pub app_name: String,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    pub actions: Vec<(String, String)>,
    pub time: chrono::DateTime<chrono::Local>,
    pub expire_at: Option<chrono::DateTime<chrono::Local>>,
    pub expanded: bool,
    pub urgency: Urgency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    Low,
    Normal,
    Critical,
}

#[derive(Debug, Clone)]
pub enum NotifEvent {
    Received(NotificationData),
    Closed(u32),
}

struct NotificationServer {
    sender: async_channel::Sender<NotifEvent>,
    next_id: Arc<AtomicU32>,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
#[allow(clippy::unused_self)]
impl NotificationServer {
    fn get_capabilities(&self) -> Vec<String> {
        vec![
            "body".to_string(),
            "body-markup".to_string(),
            "actions".to_string(),
            "icon-static".to_string(),
        ]
    }

    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, zbus::zvariant::OwnedValue>,
        expire_timeout: i32,
    ) -> zbus::fdo::Result<u32> {
        let id = if replaces_id > 0 {
            replaces_id
        } else {
            self.next_id.fetch_add(1, Ordering::SeqCst)
        };

        let action_pairs: Vec<(String, String)> = actions
            .chunks(2)
            .filter_map(|chunk| {
                let key = chunk.first()?;
                let label = chunk.get(1)?;
                Some((key.clone(), label.clone()))
            })
            .collect();

        let timeout_ms = match expire_timeout {
            t if t > 0 => t,
            t if t < 0 => 5000,
            _ => 0,
        };

        let urgency = hints
            .get("urgency")
            .and_then(|v| <u8 as TryFrom<_>>::try_from(v).ok())
            .map_or(Urgency::Normal, |u| match u {
                0 => Urgency::Low,
                2 => Urgency::Critical,
                _ => Urgency::Normal,
            });

        let expire_at = if timeout_ms > 0 {
            chrono::Local::now()
                .checked_add_signed(chrono::TimeDelta::milliseconds(i64::from(timeout_ms)))
        } else {
            None
        };

        let notif = NotificationData {
            id,
            app_name,
            app_icon,
            summary,
            body,
            actions: action_pairs,
            time: chrono::Local::now(),
            expire_at,
            expanded: false,
            urgency,
        };

        let _ = self.sender.send(NotifEvent::Received(notif)).await;
        Ok(id)
    }

    async fn close_notification(&self, id: u32) -> zbus::fdo::Result<()> {
        let _ = self.sender.send(NotifEvent::Closed(id)).await;
        Ok(())
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        (
            "obayebar".to_string(),
            "obayebar".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
            "1.2".to_string(),
        )
    }

    #[zbus(signal)]
    async fn notification_closed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn action_invoked(
        emitter: &SignalEmitter<'_>,
        id: u32,
        action_key: &str,
    ) -> zbus::Result<()>;
}

async fn run_server(sender: async_channel::Sender<NotifEvent>) {
    let server = NotificationServer {
        sender,
        next_id: Arc::new(AtomicU32::new(1)),
    };

    let result = zbus::connection::Builder::session()
        .and_then(|b| b.name("org.freedesktop.Notifications"))
        .and_then(|b| b.serve_at("/org/freedesktop/Notifications", server));

    let conn = match result {
        Ok(builder) => match builder.build().await {
            Ok(conn) => conn,
            Err(e) => {
                log::error!("Failed to build notification D-Bus connection: {e}");
                return;
            }
        },
        Err(e) => {
            log::error!("Failed to set up notification D-Bus server: {e}");
            return;
        }
    };

    log::info!("Notification daemon running on D-Bus");

    let _conn = conn;
    std::future::pending::<()>().await;
}

pub fn stream() -> impl Stream<Item = NotifEvent> {
    let (sender, receiver) = async_channel::bounded(100);

    tokio::spawn(run_server(sender));

    futures_util::stream::unfold(receiver, |rx| async move {
        rx.recv().await.ok().map(|event| (event, rx))
    })
}
