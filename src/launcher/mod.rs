pub mod desktop_entry;

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::style;
use desktop_entry::DesktopEntry;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use iced::event::{self, Event};
use iced::keyboard::{key::Named, Key};
use iced::widget::{
    button, column, container, image, mouse_area, row, scrollable, text, text_input, Column, Id,
    Space,
};
use iced::{Alignment, Border, Color, Element, Length, Subscription, Task, Theme};
use iced_layershell::to_layer_message;

const LAUNCHER_WIDTH: u32 = 600;
const LAUNCHER_HEIGHT: u32 = 500;
const MAX_VISIBLE_ENTRIES: usize = 50;
const ICON_SIZE: u32 = 24;

/// Approximate height of one entry row (icon/text + vertical padding + spacing).
const ENTRY_ROW_HEIGHT: f32 = 36.0;

const fn search_input_id() -> Id {
    Id::new("launcher-search")
}

const fn scrollable_id() -> Id {
    Id::new("launcher-entries")
}

fn focus_search() -> Task<Message> {
    iced::widget::operation::focus(search_input_id())
}

/// Approximate visible height of the scrollable entry list area.
#[allow(clippy::cast_precision_loss)]
const SCROLL_VIEWPORT_HEIGHT: f32 = LAUNCHER_HEIGHT as f32
    - style::PADDING_LARGE * 2.0
    - style::FONT_SIZE_LARGE
    - 20.0
    - style::SPACING_NORMAL;

/// Number of entries to keep visible as margin when scrolling at boundaries.
const SCROLL_MARGIN_ENTRIES: usize = 2;

pub struct Launcher {
    query: String,
    entries: Vec<DesktopEntry>,
    /// Indices into `entries`, sorted by match score or frequency.
    filtered: Vec<usize>,
    /// Index into `filtered` for the currently selected entry.
    selected: usize,
    matcher: SkimMatcherV2,
    /// Pre-loaded icon handles keyed by `desktop_id`.
    icons: HashMap<String, image::Handle>,
    /// Desktop IDs for which icon loading has already been requested.
    icons_requested: HashSet<String>,
    /// Resolved icon paths keyed by `desktop_id`, used for cache persistence.
    icon_paths: HashMap<String, PathBuf>,
    /// Launch frequency counts keyed by `desktop_id`.
    launch_counts: HashMap<String, u32>,
    /// Current vertical scroll offset (tracked for boundary-aware scrolling).
    scroll_offset: f32,
}

impl std::fmt::Debug for Launcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Launcher")
            .field("query", &self.query)
            .field("entries", &self.entries.len())
            .field("filtered", &self.filtered.len())
            .field("selected", &self.selected)
            .field("icons", &self.icons.len())
            .field("launch_counts", &self.launch_counts.len())
            .finish_non_exhaustive()
    }
}

#[to_layer_message]
#[derive(Debug, Clone)]
pub enum Message {
    SearchChanged(String),
    Launch(usize),
    Close,
    IcedEvent(Event),
    IconsLoaded(HashMap<String, image::Handle>),
    EntriesDiscovered(Vec<DesktopEntry>, HashMap<String, PathBuf>),
    ScrollChanged(scrollable::Viewport),
}

impl Launcher {
    pub fn new(
        entries: Vec<DesktopEntry>,
        icon_paths: HashMap<String, PathBuf>,
        launch_counts: HashMap<String, u32>,
        skip_background_discover: bool,
    ) -> (Self, Task<Message>) {
        let mut launcher = Self {
            query: String::new(),
            entries,
            filtered: Vec::new(),
            selected: 0,
            matcher: SkimMatcherV2::default(),
            icons: HashMap::new(),
            icons_requested: HashSet::new(),
            icon_paths,
            launch_counts,
            scroll_offset: 0.0,
        };
        launcher.update_filter();

        let load_visible = launcher.load_visible_icons();

        let mut tasks = vec![focus_search(), load_visible];

        // Re-discover entries in background to catch newly installed/removed apps
        // (skip if we just did a fresh synchronous discovery)
        if !skip_background_discover {
            tasks.push(Task::perform(
                async {
                    tokio::task::spawn_blocking(|| {
                        let entries = desktop_entry::discover_entries();
                        let icon_paths = desktop_entry::resolve_all_icon_paths(&entries);
                        (entries, icon_paths)
                    })
                    .await
                    .unwrap_or_default()
                },
                |(entries, paths)| Message::EntriesDiscovered(entries, paths),
            ));
        }

        (launcher, Task::batch(tasks))
    }

    #[must_use]
    pub fn namespace() -> String {
        "obayebar-launcher".into()
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SearchChanged(query) => {
                self.query = query;
                self.update_filter();
                self.selected = 0;
                self.scroll_offset = 0.0;
                let icons = self.load_visible_icons();
                let scroll = self.scroll_to_selected();
                Task::batch([focus_search(), icons, scroll])
            }
            Message::Launch(index) => {
                self.launch_entry(index);
                Task::none()
            }
            Message::Close => {
                std::process::exit(0);
            }
            Message::IcedEvent(event) => self.handle_event(event),
            Message::IconsLoaded(new_icons) => {
                self.icons.extend(new_icons);
                Task::none()
            }
            Message::ScrollChanged(viewport) => {
                self.scroll_offset = viewport.absolute_offset().y;
                Task::none()
            }
            Message::EntriesDiscovered(entries, icon_paths) => {
                self.entries = entries;
                self.icon_paths = icon_paths;
                self.update_filter();
                self.selected = 0;

                // Save updated cache
                desktop_entry::save_cache(&desktop_entry::LauncherCache {
                    entries: self.entries.clone(),
                    icon_paths: self.icon_paths.clone(),
                });

                // Remove icons for entries that no longer exist
                let valid: HashSet<&str> =
                    self.entries.iter().map(|e| e.desktop_id.as_str()).collect();
                self.icons.retain(|id, _| valid.contains(id.as_str()));
                self.icons_requested
                    .retain(|id| valid.contains(id.as_str()));

                // Load icons for visible entries that need them
                self.load_visible_icons()
            }
            _ => Task::none(),
        }
    }

    /// Load icons only for currently visible entries that haven't been loaded or requested yet.
    fn load_visible_icons(&mut self) -> Task<Message> {
        let needed: HashMap<String, PathBuf> = self
            .filtered
            .iter()
            .take(MAX_VISIBLE_ENTRIES)
            .filter_map(|&idx| self.entries.get(idx))
            .filter(|e| {
                !self.icons.contains_key(&e.desktop_id)
                    && !self.icons_requested.contains(&e.desktop_id)
            })
            .filter_map(|e| {
                self.icon_paths
                    .get(&e.desktop_id)
                    .map(|p| (e.desktop_id.clone(), p.clone()))
            })
            .collect();

        if needed.is_empty() {
            return Task::none();
        }

        // Mark as requested to avoid duplicate loads
        for id in needed.keys() {
            self.icons_requested.insert(id.clone());
        }

        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || load_icons_from_paths(&needed))
                    .await
                    .unwrap_or_default()
            },
            Message::IconsLoaded,
        )
    }

    /// Scroll the entry list only when the selected entry is near or past a viewport edge.
    #[allow(clippy::cast_precision_loss)]
    fn scroll_to_selected(&self) -> Task<Message> {
        let item_y = (self.selected as f32) * ENTRY_ROW_HEIGHT;
        let margin = (SCROLL_MARGIN_ENTRIES as f32) * ENTRY_ROW_HEIGHT;
        let viewport_top = self.scroll_offset;
        let viewport_bottom = self.scroll_offset + SCROLL_VIEWPORT_HEIGHT;

        // Scroll down: selected item is below viewport (minus margin)
        if item_y + ENTRY_ROW_HEIGHT > viewport_bottom - margin {
            let new_offset = item_y + ENTRY_ROW_HEIGHT + margin - SCROLL_VIEWPORT_HEIGHT;
            return iced_runtime::widget::operation::scroll_to(
                scrollable_id(),
                iced_runtime::widget::operation::AbsoluteOffset {
                    x: None,
                    y: Some(new_offset.max(0.0)),
                },
            );
        }

        // Scroll up: selected item is above viewport (plus margin)
        if item_y < viewport_top + margin {
            let new_offset = item_y - margin;
            return iced_runtime::widget::operation::scroll_to(
                scrollable_id(),
                iced_runtime::widget::operation::AbsoluteOffset {
                    x: None,
                    y: Some(new_offset.max(0.0)),
                },
            );
        }

        Task::none()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let search = text_input("Search applications...", &self.query)
            .id(search_input_id())
            .on_input(Message::SearchChanged)
            .size(style::FONT_SIZE_LARGE)
            .padding(style::PADDING_NORMAL);

        let entries: Column<'_, Message> = self
            .filtered
            .iter()
            .take(MAX_VISIBLE_ENTRIES)
            .enumerate()
            .fold(
                Column::new().spacing(2.0),
                |col, (visual_idx, &entry_idx)| {
                    col.push(self.entry_button(entry_idx, visual_idx == self.selected))
                },
            );

        let content = column![
            search,
            scrollable(entries)
                .id(scrollable_id())
                .on_scroll(Message::ScrollChanged)
                .height(Length::Fill),
        ]
        .spacing(style::SPACING_NORMAL)
        .padding(style::PADDING_LARGE)
        .width(Length::Fill)
        .height(Length::Fill);

        let card = container(content)
            .width(Length::Fixed(f32::from(
                u16::try_from(LAUNCHER_WIDTH).unwrap_or(600),
            )))
            .height(Length::Fixed(f32::from(
                u16::try_from(LAUNCHER_HEIGHT).unwrap_or(500),
            )))
            .style(|_theme| container::Style {
                background: Some(iced::Background::Color(style::with_alpha(
                    style::M3_SURFACE_CONTAINER_LOW,
                    0.95,
                ))),
                border: Border {
                    radius: style::ROUNDING_EXTRA_SMALL.into(),
                    ..Border::default()
                },
                ..container::Style::default()
            });

        mouse_area(
            container(card)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .style(|_theme| container::Style {
                    background: Some(iced::Background::Color(Color {
                        a: 0.3,
                        ..Color::BLACK
                    })),
                    ..container::Style::default()
                }),
        )
        .on_press(Message::Close)
        .into()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        // Use listen_with to receive keyboard events even when captured by the
        // focused text_input (Escape, arrows, Enter would otherwise be swallowed).
        event::listen_with(|event, _status, _id| match event {
            Event::Keyboard(_) | Event::Window(_) => Some(Message::IcedEvent(event)),
            _ => None,
        })
    }

    fn entry_button(&self, entry_idx: usize, is_selected: bool) -> Element<'_, Message> {
        let Some(entry) = self.entries.get(entry_idx) else {
            return Space::new().into();
        };

        let bg = if is_selected {
            style::with_alpha(style::M3_PRIMARY, 0.15)
        } else {
            Color::TRANSPARENT
        };

        let text_color = if is_selected {
            style::M3_PRIMARY
        } else {
            style::M3_ON_SURFACE
        };

        let name = text(&entry.name)
            .size(style::FONT_SIZE_NORMAL)
            .color(text_color);

        let mut entry_row = row![]
            .spacing(style::SPACING_SMALL)
            .align_y(Alignment::Center);

        // Add icon if available (keyed by desktop_id)
        if let Some(handle) = self.icons.get(&entry.desktop_id) {
            entry_row = entry_row.push(
                image(handle.clone())
                    .width(Length::Fixed(f32::from(
                        u16::try_from(ICON_SIZE).unwrap_or(24),
                    )))
                    .height(Length::Fixed(f32::from(
                        u16::try_from(ICON_SIZE).unwrap_or(24),
                    )))
                    .content_fit(iced::ContentFit::Contain),
            );
        }

        entry_row = entry_row.push(name);

        if let Some(ref comment) = entry.comment {
            entry_row = entry_row.push(
                text(comment)
                    .size(style::FONT_SIZE_SMALL)
                    .color(style::M3_ON_SURFACE_VARIANT),
            );
        }

        button(entry_row.width(Length::Fill))
            .on_press(Message::Launch(entry_idx))
            .style(move |_theme, status| {
                let hover = matches!(status, button::Status::Hovered | button::Status::Pressed);
                let bg_color = if hover {
                    style::with_alpha(style::M3_ON_SURFACE, 0.08)
                } else {
                    bg
                };
                button::Style {
                    background: Some(iced::Background::Color(bg_color)),
                    text_color,
                    border: Border {
                        radius: style::ROUNDING_EXTRA_SMALL.into(),
                        ..Border::default()
                    },
                    shadow: iced::Shadow::default(),
                    snap: false,
                }
            })
            .padding([style::PADDING_SMALL, style::PADDING_NORMAL])
            .width(Length::Fill)
            .into()
    }

    fn update_filter(&mut self) {
        if self.query.is_empty() {
            // Sort by launch frequency (descending), then name (ascending)
            let mut indices: Vec<usize> = (0..self.entries.len()).collect();
            indices.sort_by(|&a, &b| {
                let count_a = self
                    .entries
                    .get(a)
                    .and_then(|e| self.launch_counts.get(&e.desktop_id))
                    .copied()
                    .unwrap_or(0);
                let count_b = self
                    .entries
                    .get(b)
                    .and_then(|e| self.launch_counts.get(&e.desktop_id))
                    .copied()
                    .unwrap_or(0);
                count_b.cmp(&count_a).then_with(|| {
                    let name_a = self
                        .entries
                        .get(a)
                        .map_or_else(String::new, |e| e.name.to_lowercase());
                    let name_b = self
                        .entries
                        .get(b)
                        .map_or_else(String::new, |e| e.name.to_lowercase());
                    name_a.cmp(&name_b)
                })
            });
            self.filtered = indices;
            return;
        }

        let mut scored: Vec<(usize, i64)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(idx, entry)| {
                self.matcher
                    .fuzzy_match(&entry.search_text, &self.query)
                    .map(|score| {
                        // Boost frequently launched apps (capped to avoid dominating match quality)
                        let freq_bonus = i64::from(
                            self.launch_counts
                                .get(&entry.desktop_id)
                                .copied()
                                .unwrap_or(0)
                                .min(20),
                        )
                        .saturating_mul(5);
                        (idx, score.saturating_add(freq_bonus))
                    })
            })
            .collect();

        scored.sort_by_key(|&(_, score)| Reverse(score));
        self.filtered = scored.into_iter().map(|(idx, _)| idx).collect();
    }

    fn handle_event(&mut self, event: Event) -> Task<Message> {
        match event {
            Event::Window(iced::window::Event::Unfocused) => {
                std::process::exit(0);
            }
            Event::Keyboard(iced::keyboard::Event::KeyPressed { key, .. }) => match key {
                Key::Named(Named::Escape) => {
                    std::process::exit(0);
                }
                Key::Named(Named::ArrowDown) if !self.filtered.is_empty() => {
                    let max = self
                        .filtered
                        .len()
                        .min(MAX_VISIBLE_ENTRIES)
                        .saturating_sub(1);
                    self.selected = (self.selected.saturating_add(1)).min(max);
                    return Task::batch([focus_search(), self.scroll_to_selected()]);
                }
                Key::Named(Named::ArrowUp) => {
                    self.selected = self.selected.saturating_sub(1);
                    return Task::batch([focus_search(), self.scroll_to_selected()]);
                }
                Key::Named(Named::Enter) => {
                    if let Some(&entry_idx) = self.filtered.get(self.selected) {
                        self.launch_entry(entry_idx);
                    }
                }
                _ => {}
            },
            _ => {}
        }
        // Always keep focus on the search bar
        focus_search()
    }

    fn launch_entry(&mut self, entry_idx: usize) {
        let Some(entry) = self.entries.get(entry_idx) else {
            return;
        };
        let desktop_id = entry.desktop_id.clone();
        let exec = entry.exec.clone();
        let name = entry.name.clone();

        // Track launch frequency
        let count = self.launch_counts.entry(desktop_id).or_insert(0);
        *count = count.saturating_add(1);
        desktop_entry::save_launch_counts(&self.launch_counts);

        if let Err(err) = desktop_entry::launch(&exec) {
            log::error!("Failed to launch {name}: {err}");
        }
        std::process::exit(0);
    }
}

/// Load icons from binary RGBA cache, falling back to decode + resize from source files.
/// Supports PNG, JPEG, GIF, BMP (via `image` crate) and SVG (via `resvg`).
/// Decoded icons are saved to the binary cache for future launches.
/// Cache is invalidated when the source path changes (e.g. after NixOS rebuild).
fn load_icons_from_paths(icon_paths: &HashMap<String, PathBuf>) -> HashMap<String, image::Handle> {
    let rgba_cache_dir = desktop_entry::icon_cache_dir();
    let expected_len = (ICON_SIZE as usize)
        .saturating_mul(ICON_SIZE as usize)
        .saturating_mul(4);
    let mut icons = HashMap::new();

    for (desktop_id, source_path) in icon_paths {
        // Try pre-resized RGBA cache first (no decode/resize needed).
        // A companion .path file stores the source path used to generate the cache;
        // if it doesn't match the current source, the cache is stale.
        if let Some(ref dir) = rgba_cache_dir {
            let cached = dir.join(format!("{desktop_id}.rgba"));
            let path_file = dir.join(format!("{desktop_id}.path"));
            let path_matches = std::fs::read_to_string(&path_file)
                .ok()
                .is_some_and(|p| p == source_path.to_string_lossy());
            if path_matches {
                if let Ok(data) = std::fs::read(&cached) {
                    if data.len() == expected_len {
                        icons.insert(
                            desktop_id.clone(),
                            image::Handle::from_rgba(ICON_SIZE, ICON_SIZE, data),
                        );
                        continue;
                    }
                }
            }
        }

        // Decode from source file
        let Some(raw) = decode_icon(source_path) else {
            continue;
        };

        // Save to binary cache for next launch
        if let Some(ref dir) = rgba_cache_dir {
            std::fs::create_dir_all(dir).ok();
            std::fs::write(dir.join(format!("{desktop_id}.rgba")), &raw).ok();
            std::fs::write(
                dir.join(format!("{desktop_id}.path")),
                source_path.to_string_lossy().as_bytes(),
            )
            .ok();
        }

        icons.insert(
            desktop_id.clone(),
            image::Handle::from_rgba(ICON_SIZE, ICON_SIZE, raw),
        );
    }
    log::info!("Loaded {} app icons", icons.len());
    icons
}

/// Decode an icon file to `ICON_SIZE`×`ICON_SIZE` RGBA bytes. Supports raster and SVG.
fn decode_icon(path: &std::path::Path) -> Option<Vec<u8>> {
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

/// Decode a raster image (PNG, JPEG, etc.) and resize to `ICON_SIZE`×`ICON_SIZE` RGBA bytes.
fn decode_raster(data: &[u8], path: &std::path::Path) -> Option<Vec<u8>> {
    let Ok(img) = ::image::load_from_memory(data) else {
        log::warn!("Failed to decode icon: {}", path.display());
        return None;
    };
    let resized = img.resize_exact(
        ICON_SIZE,
        ICON_SIZE,
        ::image::imageops::FilterType::Triangle,
    );
    Some(resized.to_rgba8().into_raw())
}

pub fn theme(_launcher: &Launcher, theme: &Theme) -> iced::theme::Style {
    iced::theme::Style {
        background_color: Color::TRANSPARENT,
        text_color: theme.palette().text,
    }
}

pub fn theme_fn(_launcher: &Launcher) -> Theme {
    style::m3_theme("obayebar-launcher-dark")
}
