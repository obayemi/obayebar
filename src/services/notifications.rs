use futures_util::Stream;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use zbus::object_server::{InterfaceRef, SignalEmitter};

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

    async fn close_notification(
        &self,
        id: u32,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        let _ = self.sender.send(NotifEvent::Closed(id)).await;
        // Emit from the method's own emitter rather than re-entering
        // `object_server().interface()`, which would contend with this call's
        // live interface borrow. Emitting here also covers ids the popup list
        // has already expired, which the app-side path never sees.
        if let Err(e) = Self::notification_closed(&emitter, id, close_reason::BY_CALL).await {
            log::warn!("notifications: NotificationClosed signal failed: {e}");
        }
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

/// `NotificationClosed` reason codes from the freedesktop notification spec.
/// Every path that ends a notification's life must report one of these, or
/// clients that wait for the signal — `notify-send --wait`, and the per-
/// notification state Electron and Thunderbird keep — never learn it is over.
pub mod close_reason {
    /// The notification's own timeout elapsed.
    pub const EXPIRED: u32 = 1;
    /// The user dismissed it (right-click, or activating an action).
    pub const DISMISSED: u32 = 2;
    /// A client asked for it to close via `CloseNotification`.
    pub const BY_CALL: u32 = 3;
}

/// Emit `NotificationClosed` for one notification. Safe to call for an id the
/// server has already forgotten — the signal is fire-and-forget, and a client
/// waiting on a stale id would rather see a late close than none.
pub fn emit_closed(id: u32, reason: u32) {
    tokio::spawn(async move {
        let Some(iface) = interface_ref("NotificationClosed").await else {
            return;
        };
        log_emit(
            "NotificationClosed",
            NotificationServer::notification_closed(iface.signal_emitter(), id, reason).await,
        );
    });
}

/// Emit `ActionInvoked` for the "default" action, then close the notification.
pub fn invoke_action(id: u32, action_key: String) {
    tokio::spawn(async move {
        let Some(iface) = interface_ref("ActionInvoked").await else {
            return;
        };
        let emitter = iface.signal_emitter();
        log_emit(
            "ActionInvoked",
            NotificationServer::action_invoked(emitter, id, &action_key).await,
        );
        log_emit(
            "NotificationClosed",
            NotificationServer::notification_closed(emitter, id, close_reason::DISMISSED).await,
        );
    });
}

/// Look up the server's interface ref so the caller can emit a signal on it.
/// Logs rather than swallowing both the "no connection yet" case and a failed
/// lookup, so a missing signal is never silent.
async fn interface_ref(what: &str) -> Option<InterfaceRef<NotificationServer>> {
    let Some(conn) = NOTIF_CONN.get() else {
        log::warn!("notifications: no D-Bus connection to emit {what}");
        return None;
    };
    match conn
        .object_server()
        .interface::<_, NotificationServer>("/org/freedesktop/Notifications")
        .await
    {
        Ok(iface) => Some(iface),
        Err(e) => {
            log::warn!("notifications: no interface ref to emit {what}: {e}");
            None
        }
    }
}

/// Report a failed signal emission instead of discarding it.
fn log_emit(what: &str, result: zbus::Result<()>) {
    if let Err(e) = result {
        log::warn!("notifications: {what} signal failed: {e}");
    }
}

/// Extract image data from notification hints.
///
/// Priority per the freedesktop spec: `image-data` > `image-path` > `app_icon` (not handled here).
/// The `image-data` hint is a `(iiibiiay)` structure: width, height, rowstride, `has_alpha`,
/// `bits_per_sample`, channels, pixel data. Its header is validated by
/// [`ImageGeometry::validate`] before a single byte is allocated.
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

/// Largest `image-data` icon we will decode, per side. The popup renders icons
/// at `style::NOTIF_ICON_SIZE`, so anything beyond this is either malformed or
/// hostile — and the header fields arrive as attacker-controlled `i32`s from
/// any client on the session bus, so an unbounded allocation is reachable
/// without a cooperating sender.
const MAX_IMAGE_DIM: u32 = 4096;

/// Why an `image-data` payload was rejected. Carried instead of a bare `None`
/// so a dropped icon is diagnosable in the log rather than silently missing.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
enum ImageDataError {
    #[error("header field {field} is not the expected type")]
    BadField { field: &'static str },
    #[error("dimensions {width}x{height} outside 1..={max}")]
    BadDimensions { width: i64, height: i64, max: u32 },
    #[error("{bits} bits per sample, only 8 is supported")]
    BadSampleDepth { bits: i64 },
    #[error("{channels} channels with has_alpha={has_alpha}, expected {expected}")]
    BadChannels {
        channels: i64,
        has_alpha: bool,
        expected: u32,
    },
    #[error("rowstride {rowstride} shorter than a {needed}-byte row")]
    ShortRowstride { rowstride: u32, needed: u32 },
    #[error("payload is {got} bytes, geometry needs {needed}")]
    ShortPayload { got: usize, needed: usize },
    #[error("geometry overflows the address space")]
    Overflow,
}

/// Validated geometry of an `image-data` payload. Holding one is proof that
/// every byte the conversion loop reads is in bounds and that the RGBA output
/// length fits in a `usize`, so the loop needs no further arithmetic guards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ImageGeometry {
    width: u32,
    height: u32,
    rowstride: u32,
    channels: u32,
    has_alpha: bool,
    /// `width * height * 4`, pre-checked.
    rgba_len: usize,
}

impl ImageGeometry {
    /// Bytes of real pixel data in one row, excluding any stride padding.
    const fn row_bytes(self) -> usize {
        // width <= MAX_IMAGE_DIM and channels <= 4, so this cannot overflow.
        (self.width as usize).saturating_mul(self.channels as usize)
    }

    /// Check the raw `(iiibiiay)` header against the payload it describes.
    ///
    /// Every bound the conversion loop depends on is established here: the
    /// dimensions are capped, the sample depth is the 8 bits our byte indexing
    /// assumes, `channels` agrees with `has_alpha`, the stride covers a row,
    /// and the payload is long enough for the last row's real pixels (trailing
    /// stride padding on the final row is not required, which is what senders
    /// that size their buffer exactly produce).
    fn validate(
        width: i64,
        height: i64,
        rowstride: i64,
        has_alpha: bool,
        bits_per_sample: i64,
        channels: i64,
        data_len: usize,
    ) -> Result<Self, ImageDataError> {
        let max = i64::from(MAX_IMAGE_DIM);
        if width < 1 || width > max || height < 1 || height > max {
            return Err(ImageDataError::BadDimensions {
                width,
                height,
                max: MAX_IMAGE_DIM,
            });
        }
        if bits_per_sample != 8 {
            return Err(ImageDataError::BadSampleDepth {
                bits: bits_per_sample,
            });
        }
        let expected = if has_alpha { 4 } else { 3 };
        if channels != i64::from(expected) {
            return Err(ImageDataError::BadChannels {
                channels,
                has_alpha,
                expected,
            });
        }

        // Every cast below is now provably in range: the dimensions passed the
        // cap above and `channels` is 3 or 4.
        let width = u32::try_from(width).map_err(|_| ImageDataError::Overflow)?;
        let height = u32::try_from(height).map_err(|_| ImageDataError::Overflow)?;
        let channels = u32::try_from(channels).map_err(|_| ImageDataError::Overflow)?;

        let row_bytes = width
            .checked_mul(channels)
            .ok_or(ImageDataError::Overflow)?;
        let rowstride = u32::try_from(rowstride).map_err(|_| ImageDataError::ShortRowstride {
            rowstride: 0,
            needed: row_bytes,
        })?;
        if rowstride < row_bytes {
            return Err(ImageDataError::ShortRowstride {
                rowstride,
                needed: row_bytes,
            });
        }

        // Full strides for every row but the last, then that row's real pixels.
        let needed = (rowstride as usize)
            .checked_mul((height as usize).saturating_sub(1))
            .and_then(|n| n.checked_add(row_bytes as usize))
            .ok_or(ImageDataError::Overflow)?;
        if data_len < needed {
            return Err(ImageDataError::ShortPayload {
                got: data_len,
                needed,
            });
        }

        let rgba_len = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or(ImageDataError::Overflow)?;

        Ok(Self {
            width,
            height,
            rowstride,
            channels,
            has_alpha,
            rgba_len,
        })
    }
}

/// Expand a validated payload to tightly packed RGBA.
fn to_rgba(geom: ImageGeometry, data: &[u8]) -> Vec<u8> {
    let row_bytes = geom.row_bytes();
    let mut rgba = Vec::with_capacity(geom.rgba_len);
    for y in 0..geom.height {
        let row_start = (y as usize).saturating_mul(geom.rowstride as usize);
        // `validate` proved this range is in bounds for every row.
        let Some(row) = data.get(row_start..row_start.saturating_add(row_bytes)) else {
            break;
        };
        for px in row.chunks_exact(geom.channels as usize) {
            // `chunks_exact` yields exactly `channels` bytes, and `validate`
            // proved `channels` is 3 or 4, so this pattern always matches.
            let [r, g, b, rest @ ..] = px else { continue };
            let a = if geom.has_alpha {
                rest.first().copied().unwrap_or(u8::MAX)
            } else {
                u8::MAX
            };
            rgba.extend_from_slice(&[*r, *g, *b, a]);
        }
    }
    rgba
}

/// Parse the `(iiibiiay)` image-data structure from a D-Bus variant.
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

    // Widen every integer header field to i64 so validation sees the sender's
    // real value — including negatives — instead of a wrapped one.
    let int_field = |index: usize, name: &'static str| -> Result<i64, ImageDataError> {
        fields
            .get(index)
            .ok_or(ImageDataError::BadField { field: name })
            .and_then(|v| {
                i32::try_from(v)
                    .map(i64::from)
                    .map_err(|_| ImageDataError::BadField { field: name })
            })
    };

    let parsed = int_field(0, "width").and_then(|width| {
        let height = int_field(1, "height")?;
        let rowstride = int_field(2, "rowstride")?;
        let has_alpha = fields
            .get(3)
            .ok_or(ImageDataError::BadField { field: "has_alpha" })
            .and_then(|v| {
                bool::try_from(v).map_err(|_| ImageDataError::BadField { field: "has_alpha" })
            })?;
        let bits_per_sample = int_field(4, "bits_per_sample")?;
        let channels = int_field(5, "channels")?;
        let data: Vec<u8> = match fields.get(6) {
            Some(Value::Array(arr)) => arr.iter().filter_map(|v| u8::try_from(v).ok()).collect(),
            _ => return Err(ImageDataError::BadField { field: "data" }),
        };
        let geom = ImageGeometry::validate(
            width,
            height,
            rowstride,
            has_alpha,
            bits_per_sample,
            channels,
            data.len(),
        )?;
        Ok((geom, data))
    });

    match parsed {
        Ok((geom, data)) => Some(NotificationImage {
            width: geom.width,
            height: geom.height,
            rgba: to_rgba(geom, &data),
        }),
        Err(err) => {
            log::warn!("notifications: rejected image-data hint: {err}");
            None
        }
    }
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

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::{to_rgba, ImageDataError, ImageGeometry, MAX_IMAGE_DIM};

    /// A 2x2 RGBA payload with no stride padding.
    fn rgba_2x2() -> Vec<u8> {
        (0..16u8).collect()
    }

    #[test]
    fn accepts_tightly_packed_rgba() {
        let geom = ImageGeometry::validate(2, 2, 8, true, 8, 4, 16).expect("valid");
        assert_eq!(geom.rgba_len, 16);
        assert_eq!(to_rgba(geom, &rgba_2x2()), rgba_2x2());
    }

    #[test]
    fn expands_rgb_to_rgba_with_opaque_alpha() {
        // 2x1 RGB: two pixels, three bytes each.
        let geom = ImageGeometry::validate(2, 1, 6, false, 8, 3, 6).expect("valid");
        let out = to_rgba(geom, &[1, 2, 3, 4, 5, 6]);
        assert_eq!(out, vec![1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn skips_stride_padding_between_rows() {
        // 1x2 RGB with a 4-byte stride: one padding byte per row.
        let geom = ImageGeometry::validate(1, 2, 4, false, 8, 3, 7).expect("valid");
        let out = to_rgba(geom, &[1, 2, 3, 99, 4, 5, 6]);
        assert_eq!(out, vec![1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn rejects_negative_dimensions_instead_of_wrapping_them() {
        // The pre-fix code cast -1 to 4_294_967_295 and allocated on it.
        let err = ImageGeometry::validate(-1, 8, 32, true, 8, 4, 4).expect_err("negative width");
        assert_eq!(
            err,
            ImageDataError::BadDimensions {
                width: -1,
                height: 8,
                max: MAX_IMAGE_DIM,
            }
        );
    }

    #[test]
    fn rejects_dimensions_over_the_cap() {
        // 30000x30000 with a 4-byte payload: the allocation-bomb shape.
        let err = ImageGeometry::validate(30_000, 30_000, 0, true, 8, 0, 4).expect_err("over cap");
        assert!(matches!(err, ImageDataError::BadDimensions { .. }));
    }

    #[test]
    fn rejects_zero_channels_so_pixel_start_cannot_stall() {
        // channels == 0 made every pixel_start identical, so a 4-byte payload
        // satisfied every bounds check while the loop pushed width*height*4.
        let err = ImageGeometry::validate(64, 64, 0, true, 8, 0, 4).expect_err("zero channels");
        assert_eq!(
            err,
            ImageDataError::BadChannels {
                channels: 0,
                has_alpha: true,
                expected: 4,
            }
        );
    }

    #[test]
    fn rejects_channel_count_disagreeing_with_has_alpha() {
        let err = ImageGeometry::validate(2, 2, 8, false, 8, 4, 16).expect_err("rgb with 4 ch");
        assert_eq!(
            err,
            ImageDataError::BadChannels {
                channels: 4,
                has_alpha: false,
                expected: 3,
            }
        );
    }

    #[test]
    fn rejects_non_eight_bit_samples() {
        let err = ImageGeometry::validate(2, 2, 8, true, 16, 4, 16).expect_err("16bpp");
        assert_eq!(err, ImageDataError::BadSampleDepth { bits: 16 });
    }

    #[test]
    fn rejects_rowstride_shorter_than_a_row() {
        let err = ImageGeometry::validate(4, 2, 8, true, 8, 4, 64).expect_err("short stride");
        assert_eq!(
            err,
            ImageDataError::ShortRowstride {
                rowstride: 8,
                needed: 16,
            }
        );
    }

    #[test]
    fn rejects_payload_shorter_than_the_geometry() {
        let err = ImageGeometry::validate(2, 2, 8, true, 8, 4, 15).expect_err("short payload");
        assert_eq!(
            err,
            ImageDataError::ShortPayload {
                got: 15,
                needed: 16,
            }
        );
    }

    #[test]
    fn final_row_needs_no_stride_padding() {
        // 2x2 RGBA, stride 12 (4 bytes padding): rows 0..1 need the full
        // stride, the last row only its 8 real bytes.
        ImageGeometry::validate(2, 2, 12, true, 8, 4, 20).expect("exact-fit buffer");
        ImageGeometry::validate(2, 2, 12, true, 8, 4, 19).expect_err("one byte short");
    }

    #[test]
    fn accepts_the_largest_allowed_icon() {
        let side = i64::from(MAX_IMAGE_DIM);
        let stride = side * 4;
        let len = usize::try_from(stride * side).expect("fits");
        let geom = ImageGeometry::validate(side, side, stride, true, 8, 4, len).expect("at cap");
        assert_eq!(geom.rgba_len, len);
    }
}
