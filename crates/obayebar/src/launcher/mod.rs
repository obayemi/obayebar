//! The application launcher, drawn by the bar.
//!
//! This used to be a separate process: every keypress paid for a fork, a 30 MB
//! dynamic link, wgpu device creation and a fresh desktop-entry scan before it
//! could show a list. Living inside the bar daemon means the entry list is
//! already parsed, the icons are already decoded, and showing the surface costs
//! one frame. `obayebar-launcher` is now a shim that pokes the bar's control
//! socket.

pub mod desktop_entry;
pub mod icons;
pub mod watch;

use std::cmp::Reverse;
use std::collections::HashMap;

use crate::style;
use desktop_entry::DesktopEntry;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use iced::event::Event;
use iced::keyboard::{key::Named, Key};
use iced::widget::{
    button, column, container, image, mouse_area, row, scrollable, text, text_input, Column, Id,
    Space,
};
use iced::{Alignment, Border, Color, Element, Length, Task};
use icons::ICON_SIZE;

pub const LAUNCHER_WIDTH: u32 = 600;
pub const LAUNCHER_HEIGHT: u32 = 500;

/// Layer-shell namespace for the launcher surface, so `j/layers` can tell it
/// from a bar or a panel and a Hyprland `layerrule` can target it alone.
pub const NAMESPACE: &str = "obayebar-launcher";

const MAX_VISIBLE_ENTRIES: usize = 50;

/// Approximate height of one entry row (icon/text + vertical padding + spacing).
const ENTRY_ROW_HEIGHT: f32 = 36.0;

/// Approximate visible height of the scrollable entry list area.
#[allow(clippy::cast_precision_loss)]
const SCROLL_VIEWPORT_HEIGHT: f32 = LAUNCHER_HEIGHT as f32
    - style::PADDING_LARGE * 2.0
    - style::FONT_SIZE_LARGE
    - 20.0
    - style::SPACING_NORMAL;

/// Number of entries to keep visible as margin when scrolling at boundaries.
const SCROLL_MARGIN_ENTRIES: usize = 2;

const fn search_input_id() -> Id {
    Id::new("launcher-search")
}

const fn scrollable_id() -> Id {
    Id::new("launcher-entries")
}

fn focus_search() -> Task<Message> {
    iced::widget::operation::focus(search_input_id())
}

/// What the launcher wants after handling a message: some follow-up work, and
/// whether the surface should go away.
///
/// The surface is owned by the bar, so "close" cannot be a `process::exit` any
/// more — it has to be said out loud and acted on by the owner.
#[derive(Debug)]
pub struct Response {
    pub task: Task<Message>,
    pub dismiss: bool,
}

impl Response {
    fn stay(task: Task<Message>) -> Self {
        Self {
            task,
            dismiss: false,
        }
    }

    fn none() -> Self {
        Self::stay(Task::none())
    }

    fn dismiss() -> Self {
        Self {
            task: Task::none(),
            dismiss: true,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    SearchChanged(String),
    /// Launch the entry at this index into `entries`.
    Launch(usize),
    /// Close without launching anything.
    Dismiss,
    IconsLoaded(HashMap<String, image::Handle>),
    ScrollChanged(scrollable::Viewport),
}

pub struct Launcher {
    query: String,
    entries: Vec<DesktopEntry>,
    /// Indices into `entries`, sorted by match score or frequency.
    filtered: Vec<usize>,
    /// Index into `filtered` for the currently selected entry.
    selected: usize,
    matcher: SkimMatcherV2,
    /// Decoded icons keyed by desktop ID.
    icons: HashMap<String, image::Handle>,
    /// Launch frequency counts keyed by desktop ID.
    launch_counts: HashMap<String, u32>,
    /// Current vertical scroll offset (tracked for boundary-aware scrolling).
    scroll_offset: f32,
    /// Whether this surface has held keyboard focus since it was opened.
    ///
    /// Dismissing on `Unfocused` is what closes the launcher when something
    /// else takes the screen — but a surface that has never been focused can
    /// report `Unfocused` while it is still being mapped, and acting on that
    /// would close the launcher the instant it opened.
    focused: bool,
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

impl Default for Launcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Launcher {
    #[must_use]
    pub fn new() -> Self {
        let mut launcher = Self {
            query: String::new(),
            entries: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            matcher: SkimMatcherV2::default(),
            icons: HashMap::new(),
            launch_counts: desktop_entry::load_launch_counts(),
            scroll_offset: 0.0,
            focused: false,
        };
        launcher.update_filter();
        launcher
    }

    /// Adopt a freshly scanned entry list.
    ///
    /// The selected *application* is preserved rather than the selected index.
    /// A rescan can land while the user is typing or arrowing through results,
    /// and resetting to the top then means Enter launches something they were
    /// not pointing at.
    pub fn set_entries(&mut self, entries: Vec<DesktopEntry>) {
        let selected_id = self.selected_entry().map(|entry| entry.id.clone());
        self.entries = entries;
        self.update_filter();
        self.selected = selected_id
            .and_then(|id| {
                self.filtered
                    .iter()
                    .position(|&idx| self.entries.get(idx).is_some_and(|e| e.id == id))
            })
            .unwrap_or(0);
        // Icons for entries that are gone are just wasted memory.
        self.icons
            .retain(|id, _| self.entries.iter().any(|entry| &entry.id == id));
    }

    pub fn set_icons(&mut self, icons: HashMap<String, image::Handle>) {
        self.icons = icons;
    }

    /// Whether any entry has been discovered yet.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clear the query and selection, as when the surface is (re)opened.
    pub fn reset(&mut self) -> Task<Message> {
        self.query.clear();
        self.selected = 0;
        self.scroll_offset = 0.0;
        self.focused = false;
        self.update_filter();
        focus_search()
    }

    pub fn update(&mut self, message: Message) -> Response {
        match message {
            Message::SearchChanged(query) => {
                self.query = query;
                self.update_filter();
                self.selected = 0;
                self.scroll_offset = 0.0;
                Response::stay(Task::batch([focus_search(), self.scroll_to_selected()]))
            }
            Message::Launch(index) => {
                self.launch_entry(index);
                Response::dismiss()
            }
            Message::Dismiss => Response::dismiss(),
            Message::IconsLoaded(icons) => {
                self.set_icons(icons);
                Response::none()
            }
            Message::ScrollChanged(viewport) => {
                self.scroll_offset = viewport.absolute_offset().y;
                Response::none()
            }
        }
    }

    /// Handle a raw window event routed to the launcher surface.
    ///
    /// Keyboard events are taken here rather than through widget bindings
    /// because the search field has focus and would otherwise swallow Escape,
    /// the arrows and Enter.
    pub fn handle_event(&mut self, event: &Event) -> Response {
        match event {
            Event::Window(iced::window::Event::Focused) => self.focused = true,
            // Losing keyboard focus means something else took over the screen;
            // an invisible launcher still holding an exclusive keyboard grab is
            // the worst possible state to leave behind.
            Event::Window(iced::window::Event::Unfocused) if self.focused => {
                return Response::dismiss()
            }
            Event::Keyboard(iced::keyboard::Event::KeyPressed { key, .. }) => match key {
                Key::Named(Named::Escape) => return Response::dismiss(),
                Key::Named(Named::ArrowDown) if !self.filtered.is_empty() => {
                    let max = self
                        .filtered
                        .len()
                        .min(MAX_VISIBLE_ENTRIES)
                        .saturating_sub(1);
                    self.selected = self.selected.saturating_add(1).min(max);
                    return Response::stay(Task::batch([
                        focus_search(),
                        self.scroll_to_selected(),
                    ]));
                }
                Key::Named(Named::ArrowUp) => {
                    self.selected = self.selected.saturating_sub(1);
                    return Response::stay(Task::batch([
                        focus_search(),
                        self.scroll_to_selected(),
                    ]));
                }
                Key::Named(Named::Enter) => {
                    let Some(&entry_idx) = self.filtered.get(self.selected) else {
                        return Response::none();
                    };
                    self.launch_entry(entry_idx);
                    return Response::dismiss();
                }
                _ => {}
            },
            _ => {}
        }
        // Always keep focus on the search bar.
        Response::stay(focus_search())
    }

    pub fn view(&self) -> Element<'_, Message> {
        let search = text_input("Search applications...", &self.query)
            .id(search_input_id())
            .on_input(Message::SearchChanged)
            .size(style::FONT_SIZE_LARGE)
            .padding(style::PADDING_NORMAL)
            .style(style::search_input);

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
                .direction(scrollable::Direction::Vertical(
                    scrollable::Scrollbar::new()
                        .width(6.0)
                        .scroller_width(6.0)
                        .spacing(style::SPACING_SMALL),
                ))
                .style(style::scrollbar)
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
        .on_press(Message::Dismiss)
        .into()
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

        if let Some(handle) = self.icons.get(&entry.id) {
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
            .style(style::hover_button(
                bg,
                text_color,
                style::ROUNDING_EXTRA_SMALL,
            ))
            .padding([style::PADDING_SMALL, style::PADDING_NORMAL])
            .width(Length::Fill)
            .into()
    }

    /// Scroll the entry list only when the selected entry is near or past a
    /// viewport edge.
    #[allow(clippy::cast_precision_loss)]
    fn scroll_to_selected(&self) -> Task<Message> {
        let item_y = (self.selected as f32) * ENTRY_ROW_HEIGHT;
        let margin = (SCROLL_MARGIN_ENTRIES as f32) * ENTRY_ROW_HEIGHT;
        let viewport_top = self.scroll_offset;
        let viewport_bottom = self.scroll_offset + SCROLL_VIEWPORT_HEIGHT;

        // Scroll down: selected item is below viewport (minus margin)
        if item_y + ENTRY_ROW_HEIGHT > viewport_bottom - margin {
            let new_offset = item_y + ENTRY_ROW_HEIGHT + margin - SCROLL_VIEWPORT_HEIGHT;
            return scroll_to(new_offset.max(0.0));
        }

        // Scroll up: selected item is above viewport (plus margin)
        if item_y < viewport_top + margin {
            return scroll_to((item_y - margin).max(0.0));
        }

        Task::none()
    }

    /// The entry the selection currently points at.
    fn selected_entry(&self) -> Option<&DesktopEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|&idx| self.entries.get(idx))
    }

    fn update_filter(&mut self) {
        if self.query.is_empty() {
            // Sort by launch frequency (descending), then name (ascending).
            let mut indices: Vec<usize> = (0..self.entries.len()).collect();
            indices.sort_by_key(|&idx| {
                let entry = self.entries.get(idx);
                let count = entry
                    .and_then(|e| self.launch_counts.get(&e.id))
                    .copied()
                    .unwrap_or(0);
                let name = entry.map_or_else(String::new, |e| e.name.to_lowercase());
                (Reverse(count), name)
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
                        // Boost frequently launched apps, capped so frequency
                        // cannot bury a much better textual match.
                        let freq_bonus = i64::from(
                            self.launch_counts
                                .get(&entry.id)
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

    fn launch_entry(&mut self, entry_idx: usize) {
        let Some(entry) = self.entries.get(entry_idx) else {
            return;
        };
        let id = entry.id.clone();
        let exec = entry.exec.clone();
        let terminal = entry.terminal;
        let name = entry.name.clone();

        let count = self.launch_counts.entry(id).or_insert(0);
        *count = count.saturating_add(1);
        desktop_entry::save_launch_counts(&self.launch_counts);

        if let Err(err) = desktop_entry::launch(&exec, terminal) {
            log::error!("launcher: failed to launch {name} ({err})");
        }
    }
}

fn scroll_to(offset: f32) -> Task<Message> {
    iced_runtime::widget::operation::scroll_to(
        scrollable_id(),
        iced_runtime::widget::operation::AbsoluteOffset {
            x: None,
            y: Some(offset),
        },
    )
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn entry(id: &str, name: &str) -> DesktopEntry {
        DesktopEntry {
            id: id.to_string(),
            name: name.to_string(),
            exec: id.to_string(),
            icon: None,
            comment: None,
            terminal: false,
            search_text: name.to_lowercase(),
        }
    }

    fn launcher(entries: Vec<DesktopEntry>, counts: &[(&str, u32)]) -> Launcher {
        let mut launcher = Launcher {
            query: String::new(),
            entries: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            matcher: SkimMatcherV2::default(),
            icons: HashMap::new(),
            launch_counts: counts
                .iter()
                .map(|(id, n)| ((*id).to_string(), *n))
                .collect(),
            scroll_offset: 0.0,
            focused: false,
        };
        launcher.set_entries(entries);
        launcher
    }

    fn visible_names(launcher: &Launcher) -> Vec<&str> {
        launcher
            .filtered
            .iter()
            .filter_map(|&idx| launcher.entries.get(idx))
            .map(|e| e.name.as_str())
            .collect()
    }

    #[test]
    fn an_empty_query_ranks_by_launch_frequency_then_name() {
        let launcher = launcher(
            vec![
                entry("alpha", "Alpha"),
                entry("beta", "Beta"),
                entry("gamma", "Gamma"),
            ],
            &[("gamma", 5), ("beta", 5)],
        );
        // Equal counts fall back to the name, so the order is stable rather
        // than whatever the map happened to yield.
        assert_eq!(visible_names(&launcher), ["Beta", "Gamma", "Alpha"]);
    }

    #[test]
    fn a_query_filters_to_matches_only() {
        let mut launcher = launcher(
            vec![entry("firefox", "Firefox"), entry("gimp", "GIMP")],
            &[],
        );
        launcher.update(Message::SearchChanged("fire".into()));
        assert_eq!(visible_names(&launcher), ["Firefox"]);
    }

    #[test]
    fn frequency_breaks_a_tie_between_equally_good_matches() {
        let mut launcher = launcher(
            vec![entry("code", "Code"), entry("code-oss", "Code")],
            &[("code-oss", 10)],
        );
        launcher.update(Message::SearchChanged("code".into()));
        assert_eq!(
            launcher
                .selected_entry()
                .map(|e| e.id.as_str())
                .unwrap_or_default(),
            "code-oss"
        );
    }

    #[test]
    fn a_rescan_keeps_the_selected_application() {
        // The old code reset the selection to the top whenever a background
        // rescan landed, so Enter could launch an entry the user was not on.
        let mut launcher = launcher(
            vec![entry("a", "Alpha"), entry("b", "Beta"), entry("c", "Gamma")],
            &[],
        );
        launcher.handle_event(&key_pressed(Named::ArrowDown));
        assert_eq!(launcher.selected_entry().map(|e| e.id.as_str()), Some("b"));

        // Something new installs, sorting ahead of the selection.
        launcher.set_entries(vec![
            entry("new", "Aardvark"),
            entry("a", "Alpha"),
            entry("b", "Beta"),
            entry("c", "Gamma"),
        ]);
        assert_eq!(launcher.selected_entry().map(|e| e.id.as_str()), Some("b"));
    }

    #[test]
    fn a_rescan_that_removes_the_selection_falls_back_to_the_top() {
        let mut launcher = launcher(vec![entry("a", "Alpha"), entry("b", "Beta")], &[]);
        launcher.handle_event(&key_pressed(Named::ArrowDown));
        launcher.set_entries(vec![entry("a", "Alpha")]);
        assert_eq!(launcher.selected_entry().map(|e| e.id.as_str()), Some("a"));
    }

    #[test]
    fn arrow_keys_stay_within_the_list() {
        let mut launcher = launcher(vec![entry("a", "Alpha"), entry("b", "Beta")], &[]);
        launcher.handle_event(&key_pressed(Named::ArrowUp));
        assert_eq!(launcher.selected, 0, "up at the top must not underflow");

        for _ in 0..5 {
            launcher.handle_event(&key_pressed(Named::ArrowDown));
        }
        assert_eq!(launcher.selected, 1, "down at the bottom must not overrun");
    }

    #[test]
    fn escape_dismisses_and_enter_launches_nothing_when_there_are_no_matches() {
        let mut launcher = launcher(vec![entry("a", "Alpha")], &[]);
        assert!(launcher.handle_event(&key_pressed(Named::Escape)).dismiss);

        launcher.update(Message::SearchChanged("zzzz".into()));
        // No match, so Enter has nothing to launch and must leave the surface
        // up rather than closing on a no-op.
        assert!(!launcher.handle_event(&key_pressed(Named::Enter)).dismiss);
    }

    #[test]
    fn reopening_clears_the_previous_query() {
        let mut launcher = launcher(vec![entry("a", "Alpha"), entry("b", "Beta")], &[]);
        launcher.update(Message::SearchChanged("bet".into()));
        assert_eq!(visible_names(&launcher), ["Beta"]);

        let _ = launcher.reset();
        assert_eq!(visible_names(&launcher), ["Alpha", "Beta"]);
        assert_eq!(launcher.selected, 0);
    }

    #[test]
    fn icons_for_entries_that_disappeared_are_dropped() {
        let mut launcher = launcher(vec![entry("a", "Alpha"), entry("b", "Beta")], &[]);
        launcher.set_icons(
            [("a", 1_u8), ("b", 2)]
                .into_iter()
                .map(|(id, _)| {
                    (
                        id.to_string(),
                        image::Handle::from_rgba(1, 1, vec![0, 0, 0, 0]),
                    )
                })
                .collect(),
        );
        launcher.set_entries(vec![entry("a", "Alpha")]);
        assert_eq!(launcher.icons.len(), 1);
        assert!(launcher.icons.contains_key("a"));
    }

    #[test]
    fn an_unfocus_before_the_surface_was_ever_focused_is_ignored() {
        // A surface still being mapped can report Unfocused; acting on it
        // would close the launcher the instant the keybinding opened it.
        let mut launcher = launcher(vec![entry("a", "Alpha")], &[]);
        assert!(
            !launcher
                .handle_event(&Event::Window(iced::window::Event::Unfocused))
                .dismiss
        );

        launcher.handle_event(&Event::Window(iced::window::Event::Focused));
        assert!(
            launcher
                .handle_event(&Event::Window(iced::window::Event::Unfocused))
                .dismiss
        );
    }

    fn key_pressed(named: Named) -> Event {
        Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: Key::Named(named),
            modified_key: Key::Named(named),
            physical_key: iced::keyboard::key::Physical::Unidentified(
                iced::keyboard::key::NativeCode::Unidentified,
            ),
            location: iced::keyboard::Location::Standard,
            modifiers: iced::keyboard::Modifiers::empty(),
            text: None,
            repeat: false,
        })
    }
}
