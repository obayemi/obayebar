//! Turning resolved icon files into ready-to-draw RGBA handles.
//!
//! Decoding is the expensive half of the launcher's startup — a scan resolves
//! around ninety icons, half of them SVGs that go through resvg — so the
//! results are memoised on disk as raw `ICON_SIZE`×`ICON_SIZE` RGBA. Reading
//! one back is a single `read` with no decode at all.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use iced::widget::image;

/// Pixel size icons are looked up, rasterised and drawn at.
pub const ICON_SIZE: u32 = 24;

/// Byte length of one cached icon.
const RGBA_LEN: usize = (ICON_SIZE as usize) * (ICON_SIZE as usize) * 4;

/// Directory for pre-resized RGBA icon data.
#[must_use]
pub fn cache_dir() -> Option<PathBuf> {
    obayebar_core::xdg::cache_dir().map(|d| d.join("icons"))
}

/// Decode every icon in `icon_paths`, keyed by desktop ID.
///
/// Blocking, and meant to run on a blocking thread: it reads (and on a cold
/// cache decodes and resizes) one file per entry.
#[must_use]
#[allow(clippy::implicit_hasher)]
pub fn load(icon_paths: &HashMap<String, PathBuf>) -> HashMap<String, image::Handle> {
    let dir = cache_dir();
    let mut icons = HashMap::with_capacity(icon_paths.len());
    let mut decoded = 0_usize;

    for (id, source) in icon_paths {
        if let Some(raw) = dir.as_deref().and_then(|dir| read_cached(dir, id, source)) {
            icons.insert(
                id.clone(),
                image::Handle::from_rgba(ICON_SIZE, ICON_SIZE, raw),
            );
            continue;
        }
        let Some(raw) = decode(source) else {
            continue;
        };
        decoded = decoded.saturating_add(1);
        if let Some(dir) = dir.as_deref() {
            write_cached(dir, id, source, &raw);
        }
        icons.insert(
            id.clone(),
            image::Handle::from_rgba(ICON_SIZE, ICON_SIZE, raw),
        );
    }

    log::info!(
        "launcher: {} icons ready ({decoded} decoded from source)",
        icons.len()
    );
    icons
}

/// Read `id`'s cached RGBA, if it was generated from this exact source file.
///
/// The companion `.path` file is what makes that check possible: an icon name
/// can resolve to a different file after an upgrade (on NixOS, every rebuild
/// moves it to a new store path), and the pixels for the old one are not the
/// pixels for the new one.
fn read_cached(dir: &Path, id: &str, source: &Path) -> Option<Vec<u8>> {
    let recorded = std::fs::read_to_string(dir.join(format!("{id}.path"))).ok()?;
    if recorded != source.to_string_lossy() {
        return None;
    }
    let data = std::fs::read(dir.join(format!("{id}.rgba"))).ok()?;
    (data.len() == RGBA_LEN).then_some(data)
}

fn write_cached(dir: &Path, id: &str, source: &Path, raw: &[u8]) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    if std::fs::write(dir.join(format!("{id}.rgba")), raw).is_err() {
        return;
    }
    let _ = std::fs::write(
        dir.join(format!("{id}.path")),
        source.to_string_lossy().as_bytes(),
    );
}

/// Delete cached icons for desktop IDs that no longer exist.
///
/// Without this the cache only ever grows: uninstalling an application, or an
/// upgrade that renames its entry, leaves its pixels behind forever.
pub fn prune(live_ids: &HashSet<&str>) {
    let Some(dir) = cache_dir() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for path in entries.flatten().map(|e| e.path()) {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Strip from the full name rather than using `file_stem`: a desktop ID
        // is dotted (`org.mozilla.firefox`), and a stem would cut it short.
        let Some(id) = name
            .strip_suffix(".rgba")
            .or_else(|| name.strip_suffix(".path"))
        else {
            continue;
        };
        if !live_ids.contains(id) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Decode an icon file to `ICON_SIZE`×`ICON_SIZE` RGBA bytes.
fn decode(path: &Path) -> Option<Vec<u8>> {
    let data = std::fs::read(path).ok()?;
    let is_svg = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"));

    if is_svg {
        decode_svg(&data)
    } else {
        decode_raster(&data, path)
    }
}

/// Rasterize an SVG to `ICON_SIZE`×`ICON_SIZE` RGBA bytes.
fn decode_svg(data: &[u8]) -> Option<Vec<u8>> {
    let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(ICON_SIZE, ICON_SIZE)?;
    let size = tree.size();
    let sx = f32::from(u16::try_from(ICON_SIZE).unwrap_or(24)) / size.width();
    let sy = f32::from(u16::try_from(ICON_SIZE).unwrap_or(24)) / size.height();
    let scale = sx.min(sy);
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Some(pixmap.take())
}

/// Decode a raster image (PNG, JPEG, …) and resize to `ICON_SIZE`×`ICON_SIZE`.
fn decode_raster(data: &[u8], path: &Path) -> Option<Vec<u8>> {
    let Ok(img) = ::image::load_from_memory(data) else {
        log::warn!("launcher: failed to decode icon {}", path.display());
        return None;
    };
    let resized = img.resize_exact(
        ICON_SIZE,
        ICON_SIZE,
        ::image::imageops::FilterType::Triangle,
    );
    Some(resized.to_rgba8().into_raw())
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("obayebar_icons_{tag}"));
            std::fs::remove_dir_all(&dir).ok();
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn a_cached_icon_is_reused_only_for_the_source_it_came_from() {
        let dir = ScratchDir::new("reuse");
        let raw = vec![7_u8; RGBA_LEN];
        write_cached(&dir.0, "app", Path::new("/store/a/icon.png"), &raw);

        assert_eq!(
            read_cached(&dir.0, "app", Path::new("/store/a/icon.png")),
            Some(raw)
        );
        // A rebuild moves the icon to a new store path; the old pixels are not
        // necessarily the new icon's.
        assert_eq!(
            read_cached(&dir.0, "app", Path::new("/store/b/icon.png")),
            None
        );
    }

    #[test]
    fn a_truncated_cache_file_is_rejected() {
        let dir = ScratchDir::new("truncated");
        write_cached(&dir.0, "app", Path::new("/icon.png"), &[1, 2, 3]);
        assert_eq!(read_cached(&dir.0, "app", Path::new("/icon.png")), None);
    }

    #[test]
    fn svg_rasterizes_to_one_icon_sized_frame() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><rect width="64" height="64" fill="#f00"/></svg>"##;
        let raw = decode_svg(svg).unwrap_or_else(|| panic!("should rasterize"));
        assert_eq!(raw.len(), RGBA_LEN);
    }
}
