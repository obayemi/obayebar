//! Album art for the media panel: fetched from the `mpris:artUrl` of the
//! active track, decoded off the UI thread, and cached by URL.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use iced::widget::image;

use crate::services::http;

/// Longest side of a decoded cover. The panel is a few hundred pixels wide,
/// so anything larger only costs memory.
const ART_SIZE: u32 = 512;
/// Covers kept decoded, so skipping back and forth does not refetch.
const CACHE_CAPACITY: usize = 8;
const MAX_DOWNLOAD_BYTES: usize = 8 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub enum Art {
    Loaded(image::Handle),
    /// Remembered so a broken URL is not retried on every update.
    Failed,
}

#[derive(Debug, PartialEq, Eq)]
enum ArtSource {
    File(PathBuf),
    Http(String),
}

impl ArtSource {
    fn parse(url: &str) -> Option<Self> {
        if let Some(path) = url.strip_prefix("file://") {
            let path = path.strip_prefix("localhost").unwrap_or(path);
            return Some(Self::File(PathBuf::from(percent_decode(path))));
        }
        (url.starts_with("http://") || url.starts_with("https://"))
            .then(|| Self::Http(url.to_string()))
    }
}

/// Decode the `%XX` escapes of a URI path. A malformed escape is kept as is.
fn percent_decode(raw: &str) -> String {
    let mut decoded = Vec::with_capacity(raw.len());
    let mut rest = raw.as_bytes();
    while let Some((&first, tail)) = rest.split_first() {
        if let (b'%', [hi, lo, after @ ..]) = (first, tail) {
            if let Some(byte) = hex_byte(*hi, *lo) {
                decoded.push(byte);
                rest = after;
                continue;
            }
        }
        decoded.push(first);
        rest = tail;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_byte(hi: u8, lo: u8) -> Option<u8> {
    if !(hi.is_ascii_hexdigit() && lo.is_ascii_hexdigit()) {
        return None;
    }
    std::str::from_utf8(&[hi, lo])
        .ok()
        .and_then(|digits| u8::from_str_radix(digits, 16).ok())
}

#[derive(Debug, Default)]
pub struct ArtCache {
    entries: VecDeque<(String, Art)>,
    pending: Option<String>,
}

impl ArtCache {
    #[must_use]
    pub fn get(&self, url: &str) -> Option<&Art> {
        self.entries
            .iter()
            .find_map(|(cached, art)| (cached == url).then_some(art))
    }

    /// Whether `url` has to be fetched: neither cached nor already on its way.
    /// A `true` marks it as on its way.
    pub fn request(&mut self, url: &str) -> bool {
        if self.get(url).is_some() || self.pending.as_deref() == Some(url) {
            return false;
        }
        self.pending = Some(url.to_string());
        true
    }

    /// Store a finished fetch, evicting the oldest cover past the capacity.
    pub fn insert(&mut self, url: String, art: Art) {
        if self.pending.as_ref() == Some(&url) {
            self.pending = None;
        }
        self.entries.retain(|(cached, _)| cached != &url);
        self.entries.push_back((url, art));
        while self.entries.len() > CACHE_CAPACITY {
            self.entries.pop_front();
        }
    }
}

fn decode(bytes: &[u8]) -> Option<::image::RgbaImage> {
    let img = ::image::load_from_memory(bytes).ok()?;
    let fitted = if img.width() > ART_SIZE || img.height() > ART_SIZE {
        img.thumbnail(ART_SIZE, ART_SIZE)
    } else {
        img
    };
    Some(fitted.to_rgba8())
}

/// Fetch and decode the cover at `url`. Never fails outright: a cover that
/// cannot be had is [`Art::Failed`], and the reason is logged.
pub async fn load(url: String) -> Art {
    let Some(source) = ArtSource::parse(&url) else {
        log::debug!("media: unsupported art URL {url}");
        return Art::Failed;
    };
    let bytes = match source {
        ArtSource::File(path) => read_file(path).await,
        ArtSource::Http(url) => fetch(&url).await,
    };
    let Some(bytes) = bytes else {
        return Art::Failed;
    };
    let decoded = tokio::task::spawn_blocking(move || decode(&bytes))
        .await
        .ok()
        .flatten();
    decoded.map_or_else(
        || {
            log::warn!("media: could not decode art from {url}");
            Art::Failed
        },
        |img| {
            let (width, height) = img.dimensions();
            Art::Loaded(image::Handle::from_rgba(width, height, img.into_raw()))
        },
    )
}

async fn read_file(path: PathBuf) -> Option<Vec<u8>> {
    let too_large = tokio::fs::metadata(&path)
        .await
        .is_ok_and(|meta| usize::try_from(meta.len()).map_or(true, |len| len > MAX_DOWNLOAD_BYTES));
    if too_large {
        log::warn!("media: art file {} is too large", path.display());
        return None;
    }
    tokio::fs::read(&path)
        .await
        .map_err(|e| log::warn!("media: reading art {} failed: {e}", path.display()))
        .ok()
}

fn http_client() -> Option<&'static reqwest::Client> {
    static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            http::client(REQUEST_TIMEOUT)
                .map_err(|e| log::warn!("media: failed to build HTTP client: {e}"))
                .ok()
        })
        .as_ref()
}

async fn fetch(url: &str) -> Option<Vec<u8>> {
    let mut response = http_client()?
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| log::warn!("media: fetching art {url} failed: {e}"))
        .ok()?;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| log::warn!("media: fetching art {url} failed: {e}"))
        .ok()?
    {
        if body.len().saturating_add(chunk.len()) > MAX_DOWNLOAD_BYTES {
            log::warn!("media: art at {url} is too large");
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    Some(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle() -> Art {
        Art::Loaded(image::Handle::from_rgba(1, 1, vec![0; 4]))
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = std::io::Cursor::new(Vec::new());
        ::image::RgbaImage::new(width, height)
            .write_to(&mut bytes, ::image::ImageFormat::Png)
            .unwrap_or_else(|e| unreachable!("{e}"));
        bytes.into_inner()
    }

    #[test]
    fn file_urls_become_decoded_paths() {
        assert_eq!(
            ArtSource::parse("file:///home/me/My%20Music/cover%C3%A9.jpg"),
            Some(ArtSource::File(PathBuf::from(
                "/home/me/My Music/cover\u{e9}.jpg"
            )))
        );
    }

    #[test]
    fn http_urls_are_fetched_as_given() {
        for url in ["https://i.scdn.co/image/ab67", "http://localhost/a%20b.png"] {
            assert_eq!(
                ArtSource::parse(url),
                Some(ArtSource::Http(url.to_string()))
            );
        }
    }

    #[test]
    fn other_schemes_are_not_supported() {
        assert_eq!(ArtSource::parse("data:image/png;base64,AAAA"), None);
        assert_eq!(ArtSource::parse("/no/scheme.png"), None);
    }

    #[test]
    fn malformed_escapes_are_kept_literally() {
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz%4"), "%zz%4");
        assert_eq!(percent_decode("a%2Fb"), "a/b");
    }

    #[test]
    fn a_url_is_requested_once_until_it_lands() {
        let mut cache = ArtCache::default();
        assert!(cache.request("a"));
        assert!(!cache.request("a"));
        cache.insert("a".to_string(), handle());
        assert!(!cache.request("a"));
        assert!(matches!(cache.get("a"), Some(Art::Loaded(_))));
    }

    #[test]
    fn a_failed_fetch_is_not_retried() {
        let mut cache = ArtCache::default();
        assert!(cache.request("a"));
        cache.insert("a".to_string(), Art::Failed);
        assert!(!cache.request("a"));
        assert!(matches!(cache.get("a"), Some(Art::Failed)));
    }

    #[test]
    fn a_new_track_can_be_requested_while_another_is_pending() {
        let mut cache = ArtCache::default();
        assert!(cache.request("a"));
        assert!(cache.request("b"));
    }

    #[test]
    fn the_oldest_cover_is_evicted_past_capacity() {
        let mut cache = ArtCache::default();
        for i in 0..=CACHE_CAPACITY {
            cache.insert(i.to_string(), handle());
        }
        assert!(cache.get("0").is_none());
        assert!(cache.get("1").is_some());
        assert!(cache.get(&CACHE_CAPACITY.to_string()).is_some());
    }

    #[test]
    fn decoding_downscales_large_covers_keeping_the_aspect() {
        let decoded = decode(&png(2048, 1024)).map(|img| img.dimensions());
        assert_eq!(decoded, Some((ART_SIZE, ART_SIZE / 2)));
    }

    #[test]
    fn small_covers_keep_their_size() {
        assert_eq!(
            decode(&png(64, 64)).map(|img| img.dimensions()),
            Some((64, 64))
        );
    }

    #[test]
    fn garbage_does_not_decode() {
        assert!(decode(b"not an image").is_none());
    }
}
