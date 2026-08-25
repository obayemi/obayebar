//! Turning a file on disk into the pixels `wl_shm` wants.
//!
//! Decoding is content-sniffed rather than extension-dispatched, for the same
//! reason discovery is: `image::open` picks a decoder from the file extension,
//! and a real wallpaper directory contains a `.jpe` and a `…wallpaper.jpg.png`.
//! `with_guessed_format` reads the magic bytes instead.

use std::path::{Path, PathBuf};

use image::imageops::FilterType;

/// Bytes per pixel in the `Argb8888` format we hand the compositor.
const BYTES_PER_PIXEL: usize = 4;

/// A decoded wallpaper, sized for one particular output.
#[derive(Clone)]
pub struct Wallpaper {
    /// The file this came from, so a re-assignment of the same picture at the
    /// same size can reuse it instead of decoding again.
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    /// Little-endian `Argb8888`: in memory each pixel is B, G, R, A.
    pub bgra: Vec<u8>,
}

impl std::fmt::Debug for Wallpaper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wallpaper")
            .field("path", &self.path)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.bgra.len())
            .finish()
    }
}

/// Why a wallpaper could not be prepared.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("reading {path}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("decoding {path}")]
    Decode {
        path: String,
        #[source]
        source: image::ImageError,
    },
    #[error("{path} decoded to an empty image")]
    Empty { path: String },
    #[error("{width}x{height} is too large to buffer")]
    TooLarge { width: u32, height: u32 },
}

/// Decode `path` and scale it to exactly `width`x`height`, cropping to fill.
///
/// `resize_to_fill` is the "cover" behaviour a wallpaper wants: the image keeps
/// its aspect ratio, fills the whole output, and the overflow is cropped
/// centrally. Scaling to the output's real size here rather than letting
/// anything downstream do it means the compositor gets a buffer it can put on
/// screen unchanged.
///
/// # Errors
///
/// Returns [`DecodeError`] when the file cannot be read, is not an image any
/// compiled-in decoder understands, decodes to nothing, or is too large for a
/// buffer allocation to be worth attempting.
pub fn prepare(path: &Path, width: u32, height: u32) -> Result<Wallpaper, DecodeError> {
    let display = path.display().to_string();
    if width == 0 || height == 0 {
        return Err(DecodeError::Empty { path: display });
    }

    let reader = image::ImageReader::open(path)
        .map_err(|source| DecodeError::Read {
            path: display.clone(),
            source,
        })?
        .with_guessed_format()
        .map_err(|source| DecodeError::Read {
            path: display.clone(),
            source,
        })?;

    let started = std::time::Instant::now();
    let decoded = reader.decode().map_err(|source| DecodeError::Decode {
        path: display.clone(),
        source,
    })?;
    let decoded_at = started.elapsed();

    let filled = decoded.resize_to_fill(width, height, FilterType::Lanczos3);
    let resized_at = started.elapsed();
    let rgba = filled.into_rgba8();

    let pixels = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(BYTES_PER_PIXEL))
        .ok_or(DecodeError::TooLarge { width, height })?;

    // wl_shm's Argb8888 is little-endian, so the bytes run B, G, R, A. The
    // alpha is forced opaque: a wallpaper with a transparent region would
    // otherwise show the compositor's own background through it, and every
    // caller here treats the surface as opaque anyway.
    let mut bgra = Vec::with_capacity(pixels);
    for px in rgba.pixels() {
        let [r, g, b, _] = px.0;
        bgra.extend_from_slice(&[b, g, r, 0xFF]);
    }

    if bgra.is_empty() {
        return Err(DecodeError::Empty { path: display });
    }

    log::debug!(
        "wallpaper: {display} -> {width}x{height} in {}ms (decode {}ms, resize {}ms, pack {}ms)",
        started.elapsed().as_millis(),
        decoded_at.as_millis(),
        resized_at.saturating_sub(decoded_at).as_millis(),
        started.elapsed().saturating_sub(resized_at).as_millis(),
    );

    Ok(Wallpaper {
        path: path.to_path_buf(),
        width,
        height,
        bgra,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        // Named per test and cleaned first, so a stale directory from an
        // interrupted run cannot make the next one fail.
        let dir = std::env::temp_dir().join(format!("obayebar-decode-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Write a solid-colour PNG under a deliberately misleading name.
    fn write_png(dir: &Path, name: &str, w: u32, h: u32, rgb: [u8; 3]) -> PathBuf {
        let path = dir.join(name);
        let buf = image::RgbImage::from_pixel(w, h, image::Rgb(rgb));
        buf.save_with_format(&path, image::ImageFormat::Png)
            .unwrap();
        path
    }

    #[test]
    fn decodes_regardless_of_extension() {
        let dir = scratch("ext");
        // A PNG called .jpe — the exact shape that breaks extension dispatch.
        let path = write_png(&dir, "lying.jpe", 8, 8, [10, 20, 30]);
        let out = prepare(&path, 4, 4).unwrap();
        assert_eq!((out.width, out.height), (4, 4));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scales_to_exactly_the_requested_size() {
        let dir = scratch("scale");
        // Deliberately the wrong aspect ratio, to prove it fills rather than
        // letterboxes: a 100x10 source into a 40x40 output.
        let path = write_png(&dir, "wide.png", 100, 10, [0, 0, 0]);
        let out = prepare(&path, 40, 40).unwrap();
        assert_eq!((out.width, out.height), (40, 40));
        assert_eq!(out.bgra.len(), 40 * 40 * BYTES_PER_PIXEL);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn emits_opaque_bgra_in_that_byte_order() {
        let dir = scratch("order");
        // Pure red, so a channel swap is unmissable.
        let path = write_png(&dir, "red.png", 4, 4, [255, 0, 0]);
        let out = prepare(&path, 2, 2).unwrap();
        assert_eq!(
            &out.bgra[0..4],
            &[0, 0, 255, 255],
            "little-endian Argb8888 is B,G,R,A in memory"
        );
        assert!(
            out.bgra.chunks_exact(4).all(|px| px[3] == 0xFF),
            "wallpapers must be fully opaque"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_a_zero_sized_output() {
        let dir = scratch("zero");
        let path = write_png(&dir, "a.png", 4, 4, [1, 2, 3]);
        // A configure can legitimately arrive before the size is known; asking
        // for a 0-dimension buffer must fail cleanly rather than allocate.
        assert!(prepare(&path, 0, 10).is_err());
        assert!(prepare(&path, 10, 0).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reports_a_missing_file_as_a_read_error() {
        let missing = std::env::temp_dir().join("obayebar-decode-test-nope.png");
        let _ = std::fs::remove_file(&missing);
        assert!(matches!(
            prepare(&missing, 4, 4),
            Err(DecodeError::Read { .. })
        ));
    }

    #[test]
    fn reports_a_non_image_as_a_decode_error() {
        let dir = scratch("garbage");
        let path = dir.join("notes.png");
        std::fs::write(&path, b"this is not an image").unwrap();
        // Sniffing means this fails as "not a recognised format" rather than
        // being believed because of the .png name.
        assert!(prepare(&path, 4, 4).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
