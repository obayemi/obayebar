//! Wallpaper selection, shared by the background renderer and the lock screen.
//!
//! `plan` is the pure core (ordering, dealing, interval parsing), `state` is
//! what gets persisted so both readers agree, and `hyprlock` renders the lock
//! screen's config. Discovery lives here because it is the one part that has
//! to touch the filesystem.

pub mod hyprlock;
pub mod plan;
pub mod state;

#[cfg(feature = "images")]
use std::path::{Path, PathBuf};

pub use plan::{build_order, parse_interval, plan_wallpapers, IntervalError, WallpaperPlan};
pub use state::State;

/// How many bytes to read when identifying a file.
///
/// `image::guess_format` only ever inspects a short magic-number prefix; the
/// longest signature the enabled decoders use is well inside this.
#[cfg(feature = "images")]
const SNIFF_BYTES: usize = 64;

/// Find the usable wallpapers in `dir`, sorted by path.
///
/// Files are identified by **content**, never by extension. `image::open` and
/// friends dispatch on the extension, which quietly breaks on the two shapes
/// this directory actually contains: a `.jpe` file, and one named
/// `…wallpaper.jpg.png`. Sniffing costs one short read per file and makes a
/// mislabelled wallpaper work rather than vanish.
///
/// The sort is what makes a seeded shuffle reproducible: `read_dir` returns
/// entries in filesystem order, which is neither stable nor meaningful.
#[cfg(feature = "images")]
#[must_use]
pub fn discover(dir: &Path) -> Vec<PathBuf> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            log::warn!("wallpaper: cannot read {} ({e})", dir.display());
            return Vec::new();
        }
    };

    let mut found: Vec<PathBuf> = entries
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry.path()),
            Err(e) => {
                log::warn!(
                    "wallpaper: skipping unreadable entry in {} ({e})",
                    dir.display()
                );
                None
            }
        })
        .filter(|path| path.is_file() && is_supported_image(path))
        .collect();

    found.sort();
    if found.is_empty() {
        log::warn!("wallpaper: no usable images in {}", dir.display());
    } else {
        log::info!("wallpaper: {} images in {}", found.len(), dir.display());
    }
    found
}

/// Whether the file's magic bytes name a format the build can decode.
///
/// Checked against the `image` features actually compiled in, so a WebP in the
/// directory is reported as unsupported here rather than failing later at
/// decode time with the surface already created.
#[cfg(feature = "images")]
#[must_use]
pub fn is_supported_image(path: &Path) -> bool {
    let Some(header) = read_header(path) else {
        return false;
    };
    ::image::guess_format(&header).is_ok_and(|format| {
        let supported = matches!(
            format,
            ::image::ImageFormat::Png
                | ::image::ImageFormat::Jpeg
                | ::image::ImageFormat::Gif
                | ::image::ImageFormat::Bmp
        );
        if !supported {
            log::debug!(
                "wallpaper: skipping {} ({format:?} is not compiled in)",
                path.display()
            );
        }
        supported
    })
}

#[cfg(feature = "images")]
fn read_header(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0_u8; SNIFF_BYTES];
    // A short read is fine and expected for tiny files; `guess_format` simply
    // fails to match, which is the answer we want.
    let read = file.read(&mut buf).ok()?;
    buf.truncate(read);
    Some(buf)
}

#[cfg(all(test, feature = "images"))]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    /// Smallest valid PNG: signature plus an empty IHDR-ish body. Only the
    /// 8-byte signature matters to `guess_format`.
    const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR";
    const JPEG_MAGIC: &[u8] = b"\xff\xd8\xff\xe0\x00\x10JFIF\x00";
    const GIF_MAGIC: &[u8] = b"GIF89a\x01\x00\x01\x00";
    const WEBP_MAGIC: &[u8] = b"RIFF\x24\x00\x00\x00WEBPVP8 ";

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("obayebar-wallpaper-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn identifies_by_content_not_extension() {
        let dir = scratch("content");
        // The two real shapes from the user's directory: an unusual extension
        // and a doubled one. Both must be found.
        let jpe = write(&dir, "photo.jpe", JPEG_MAGIC);
        let doubled = write(&dir, "wallpaper.jpg.png", PNG_MAGIC);
        assert!(is_supported_image(&jpe));
        assert!(is_supported_image(&doubled));

        // And a lie in the other direction must not be believed.
        let liar = write(&dir, "notes.png", b"just some text, not an image at all");
        assert!(!is_supported_image(&liar));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_formats_that_are_not_compiled_in() {
        let dir = scratch("webp");
        let webp = write(&dir, "a.webp", WEBP_MAGIC);
        assert!(
            !is_supported_image(&webp),
            "webp is a real format but this build has no decoder for it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tiny_and_empty_files_do_not_panic() {
        let dir = scratch("tiny");
        assert!(!is_supported_image(&write(&dir, "empty", b"")));
        assert!(!is_supported_image(&write(&dir, "short", b"\x89P")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_returns_sorted_images_only() {
        let dir = scratch("discover");
        write(&dir, "b.jpg", JPEG_MAGIC);
        write(&dir, "a.png", PNG_MAGIC);
        write(&dir, "c.gif", GIF_MAGIC);
        write(&dir, "readme.txt", b"not an image");
        std::fs::create_dir_all(dir.join("subdir")).unwrap();

        let found = discover(&dir);
        let names: Vec<&str> = found
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
            .collect();
        assert_eq!(
            names,
            vec!["a.png", "b.jpg", "c.gif"],
            "sorted, images only, no directories"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_of_a_missing_directory_is_empty_not_fatal() {
        let missing = std::env::temp_dir().join("obayebar-wallpaper-test-does-not-exist");
        let _ = std::fs::remove_dir_all(&missing);
        assert!(discover(&missing).is_empty());
    }
}
