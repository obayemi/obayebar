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

const fn search_input_id() -> Id {
    Id::new("launcher-search")
}

fn focus_search() -> Task<Message> {
    iced::widget::operation::focus(search_input_id())
}

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
    /// Resolved icon paths keyed by `desktop_id`, used for cache persistence.
    icon_paths: HashMap<String, PathBuf>,
    /// Launch frequency counts keyed by `desktop_id`.
    launch_counts: HashMap<String, u32>,
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
}

impl Launcher {
    pub fn new(
        entries: Vec<DesktopEntry>,
        icon_paths: HashMap<String, PathBuf>,
        launch_counts: HashMap<String, u32>,
    ) -> (Self, Task<Message>) {
        let mut launcher = Self {
            query: String::new(),
            entries,
            filtered: Vec::new(),
            selected: 0,
            matcher: SkimMatcherV2::default(),
            icons: HashMap::new(),
            icon_paths,
            launch_counts,
        };
        launcher.update_filter();

        // Load icons in background from cached paths
        let paths = launcher.icon_paths.clone();
        let load_icons = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || load_icons_from_paths(&paths))
                    .await
                    .unwrap_or_default()
            },
            Message::IconsLoaded,
        );

        // Re-discover entries in background to catch newly installed/removed apps
        let discover = Task::perform(
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
        );

        (
            launcher,
            Task::batch([focus_search(), load_icons, discover]),
        )
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
                focus_search()
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

                // Load icons for newly discovered entries
                let new_paths: HashMap<String, PathBuf> = self
                    .icon_paths
                    .iter()
                    .filter(|(id, _)| !self.icons.contains_key(id.as_str()))
                    .map(|(id, path)| (id.clone(), path.clone()))
                    .collect();

                if new_paths.is_empty() {
                    Task::none()
                } else {
                    Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || load_icons_from_paths(&new_paths))
                                .await
                                .unwrap_or_default()
                        },
                        Message::IconsLoaded,
                    )
                }
            }
            _ => Task::none(),
        }
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

        let content = column![search, scrollable(entries).height(Length::Fill),]
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
                    self.selected = (self.selected.saturating_add(1)).min(
                        self.filtered
                            .len()
                            .saturating_sub(1)
                            .min(MAX_VISIBLE_ENTRIES.saturating_sub(1)),
                    );
                }
                Key::Named(Named::ArrowUp) => {
                    self.selected = self.selected.saturating_sub(1);
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

/// Load and resize icons from resolved filesystem paths.
fn load_icons_from_paths(icon_paths: &HashMap<String, PathBuf>) -> HashMap<String, image::Handle> {
    let mut icons = HashMap::new();
    for (desktop_id, path) in icon_paths {
        let Ok(data) = std::fs::read(path) else {
            continue;
        };
        let Ok(img) = ::image::load_from_memory(&data) else {
            log::warn!("Failed to decode icon: {}", path.display());
            continue;
        };
        let resized = img.resize_exact(
            ICON_SIZE,
            ICON_SIZE,
            ::image::imageops::FilterType::Lanczos3,
        );
        let rgba = resized.to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());
        icons.insert(
            desktop_id.clone(),
            image::Handle::from_rgba(w, h, rgba.into_raw()),
        );
    }
    log::info!("Loaded {} app icons", icons.len());
    icons
}

pub fn theme(_launcher: &Launcher, theme: &Theme) -> iced::theme::Style {
    iced::theme::Style {
        background_color: Color::TRANSPARENT,
        text_color: theme.palette().text,
    }
}

pub fn theme_fn(_launcher: &Launcher) -> Theme {
    Theme::custom(
        String::from("obayebar-launcher-dark"),
        iced::theme::Palette {
            background: Color::TRANSPARENT,
            text: style::M3_ON_SURFACE,
            primary: style::M3_PRIMARY,
            success: Color::from_rgb(0.2, 0.8, 0.2),
            danger: style::M3_ERROR,
            warning: style::M3_TERTIARY,
        },
    )
}
