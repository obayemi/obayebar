//! Album art for the media panel: fetched from the `mpris:artUrl` of the
//! active track, decoded off the UI thread.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use iced::widget::image;

use crate::services::http;

/// Longest side of a decoded cover. The panel is a few hundred pixels wide,
/// so anything larger only costs memory.
const ART_SIZE: u32 = 512;
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
    Http(reqwest::Url),
}

impl ArtSource {
    fn parse(url: &str) -> Option<Self> {
        if let Some(path) = super::file_uri::local_path(url) {
            return Some(Self::File(path));
        }
        let url = reqwest::Url::parse(url).ok()?;
        matches!(url.scheme(), "http" | "https").then_some(Self::Http(url))
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
        ArtSource::Http(url) => fetch(url).await,
    };
    let Some(bytes) = bytes else {
        return Art::Failed;
    };
    let decoded = tokio::task::spawn_blocking(move || decode(&bytes))
        .await
        .ok()
        .flatten();
    let Some(img) = decoded else {
        log::warn!("media: could not decode art from {url}");
        return Art::Failed;
    };
    let (width, height) = img.dimensions();
    Art::Loaded(image::Handle::from_rgba(width, height, img.into_raw()))
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

async fn fetch(url: reqwest::Url) -> Option<Vec<u8>> {
    let mut response = http_client()?
        .get(url.clone())
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
            let parsed = ArtSource::parse(url);
            assert!(
                matches!(&parsed, Some(ArtSource::Http(parsed_url)) if parsed_url.as_str() == url)
            );
        }
    }

    #[test]
    fn other_schemes_are_not_supported() {
        assert_eq!(ArtSource::parse("data:image/png;base64,AAAA"), None);
    }

    #[test]
    fn a_file_url_naming_another_host_is_rejected() {
        assert_eq!(ArtSource::parse("file://otherhost/x"), None);
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
