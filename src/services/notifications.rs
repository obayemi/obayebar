use futures_util::Stream;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use zbus::object_server::SignalEmitter;

/// RGBA image data ready for display.
#[derive(Debug, Clone)]
pub struct NotificationImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

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
    pub image: Option<NotificationImage>,
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

/// Stored D-Bus connection for emitting signals from outside the server handler.
static NOTIF_CONN: OnceLock<zbus::Connection> = OnceLock::new();

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
            "image/rgba".to_string(),
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

        let image = extract_image(&hints);

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
            image,
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

/// Emit `ActionInvoked` signal for the "default" action, then close the notification.
pub fn invoke_action(id: u32, action_key: String) {
    tokio::spawn(async move {
        let Some(conn) = NOTIF_CONN.get() else {
            log::warn!("notifications: no D-Bus connection for ActionInvoked");
            return;
        };
        let iface_ref = conn
            .object_server()
            .interface::<_, NotificationServer>("/org/freedesktop/Notifications")
            .await;
        let Ok(iface_ref) = iface_ref else {
            log::warn!("notifications: could not get interface ref for signals");
            return;
        };
        let emitter = iface_ref.signal_emitter();
        if let Err(e) = NotificationServer::action_invoked(emitter, id, &action_key).await {
            log::warn!("notifications: ActionInvoked signal failed: {e}");
        }
        // Reason 2 = dismissed by user
        if let Err(e) = NotificationServer::notification_closed(emitter, id, 2).await {
            log::warn!("notifications: NotificationClosed signal failed: {e}");
        }
    });
}

/// Extract image data from notification hints.
///
/// Priority per the freedesktop spec: `image-data` > `image-path` > `app_icon` (not handled here).
/// The `image-data` hint is a `(iiibiiay)` structure: width, height, rowstride, `has_alpha`,
/// `bits_per_sample`, channels, pixel data.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
fn extract_image(hints: &HashMap<String, zbus::zvariant::OwnedValue>) -> Option<NotificationImage> {
    // Try image-data / image_data first
    for key in &["image-data", "image_data"] {
        if let Some(val) = hints.get(*key) {
            if let Some(img) = parse_image_data(val) {
                return Some(img);
            }
        }
    }

    // Try image-path / image_path
    for key in &["image-path", "image_path"] {
        if let Some(val) = hints.get(*key) {
            if let Ok(value) = val.downcast_ref::<zbus::zvariant::Value>() {
                if let Ok(path) = String::try_from(value) {
                    if let Some(img) = load_image_from_path(&path) {
                        return Some(img);
                    }
                }
            }
        }
    }

    None
}

/// Parse the `(iiibiiay)` image-data structure from a D-Bus variant.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
fn parse_image_data(value: &zbus::zvariant::OwnedValue) -> Option<NotificationImage> {
    use zbus::zvariant::Value;

    let structure = if let Ok(Value::Structure(s)) = value.downcast_ref::<Value>() {
        s.try_clone().ok()?
    } else {
        // Try via owned conversion
        let val: Value = value.try_into().ok()?;
        if let Value::Structure(s) = val {
            s.try_clone().ok()?
        } else {
            return None;
        }
    };

    let fields = structure.fields();
    if fields.len() < 7 {
        return None;
    }

    let width = i32::try_from(fields.first()?).ok()? as u32;
    let height = i32::try_from(fields.get(1)?).ok()? as u32;
    let rowstride = i32::try_from(fields.get(2)?).ok()? as u32;
    let has_alpha = bool::try_from(fields.get(3)?).ok()?;
    let _bits_per_sample = i32::try_from(fields.get(4)?).ok()?;
    let channels = i32::try_from(fields.get(5)?).ok()? as u32;
    let data: Vec<u8> = match fields.get(6)? {
        Value::Array(arr) => arr.iter().filter_map(|v| u8::try_from(v).ok()).collect(),
        _ => return None,
    };

    if width == 0 || height == 0 {
        return None;
    }

    // Convert to RGBA
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        let row_start = (y * rowstride) as usize;
        for x in 0..width {
            let pixel_start = row_start + (x * channels) as usize;
            let r = *data.get(pixel_start)?;
            let g = *data.get(pixel_start + 1)?;
            let b = *data.get(pixel_start + 2)?;
            let a = if has_alpha {
                *data.get(pixel_start + 3)?
            } else {
                255
            };
            rgba.push(r);
            rgba.push(g);
            rgba.push(b);
            rgba.push(a);
        }
    }

    Some(NotificationImage {
        width,
        height,
        rgba,
    })
}

/// Load an image from a file path and convert to RGBA.
fn load_image_from_path(path: &str) -> Option<NotificationImage> {
    let path = path.strip_prefix("file://").unwrap_or(path).to_string();

    let img = image::open(&path).ok()?.into_rgba8();
    let width = img.width();
    let height = img.height();
    let rgba = img.into_raw();

    Some(NotificationImage {
        width,
        height,
        rgba,
    })
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

    let _ = NOTIF_CONN.set(conn.clone());

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
