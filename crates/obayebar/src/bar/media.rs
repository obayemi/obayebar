use super::rotated_text::{render_rotated_text, truncate_with_ellipsis};
use crate::panel::PanelKind;
use crate::services::media::Track;
use crate::Message;
use ab_glyph::FontArc;
use iced::widget::{column, container, image, mouse_area, text, Column};
use iced::{Alignment, Element, Length};
use obayebar::style;

/// Longest label the bar shows before truncating it.
const MAX_LABEL_CHARS: usize = 28;

/// "title — artist" for the bar; the title alone without artists, and the
/// player's name while it has no title.
pub fn label(track: &Track, identity: &str) -> String {
    let full = match (track.title.is_empty(), track.artists.is_empty()) {
        (true, _) => identity.to_string(),
        (false, true) => track.title.clone(),
        (false, false) => format!("{} \u{2014} {}", track.title, track.artists.join(", ")),
    };
    truncate_with_ellipsis(&full, MAX_LABEL_CHARS)
}

/// Render the bar entry: a note icon over the rotated track label, or the icon
/// alone without a `label` or a vector font, since horizontal text cannot
/// fit the bar's width. Hovering or clicking opens the media panel.
pub fn view<'a>(
    label: Option<&str>,
    font: Option<&FontArc>,
    monitor: Option<String>,
) -> Element<'a, Message> {
    let icon = text(style::ICON_MUSIC_NOTE)
        .font(style::ICON_FONT)
        .size(style::FONT_SIZE_LARGE)
        .color(style::M3_PRIMARY)
        .align_x(Alignment::Center);

    let mut stack: Column<'_, Message> = column![icon]
        .spacing(style::SPACING_SMALL)
        .align_x(Alignment::Center);
    if let Some((font, label)) = font.zip(label) {
        let handle = render_rotated_text(
            font,
            label,
            style::FONT_SIZE_NORMAL,
            style::M3_ON_SURFACE_VARIANT,
        );
        stack = stack.push(image(handle).content_fit(iced::ContentFit::None));
    }

    let open_msg = Message::PanelOpen(PanelKind::Media, monitor);
    let clickable = mouse_area(stack)
        .on_press(open_msg.clone())
        .on_enter(open_msg)
        .on_exit(Message::PanelPointerLeftTrigger(PanelKind::Media));

    container(clickable)
        .padding(style::PADDING_NORMAL)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .style(style::pill_container)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, artists: &[&str]) -> Track {
        Track {
            title: title.to_string(),
            artists: artists.iter().map(ToString::to_string).collect(),
            ..Track::default()
        }
    }

    #[test]
    fn title_and_artists_are_joined_with_a_dash() {
        assert_eq!(
            label(&track("Song", &["A", "B"]), "mpv"),
            "Song \u{2014} A, B"
        );
    }

    #[test]
    fn a_track_without_artists_shows_its_title() {
        assert_eq!(label(&track("Song", &[]), "mpv"), "Song");
    }

    #[test]
    fn a_track_without_title_shows_the_player() {
        assert_eq!(label(&track("", &["A"]), "Spotify"), "Spotify");
    }

    #[test]
    fn long_labels_are_truncated() {
        let long = "x".repeat(MAX_LABEL_CHARS * 2);
        let shown = label(&track(&long, &["A"]), "mpv");
        assert_eq!(shown.chars().count(), MAX_LABEL_CHARS);
        assert!(shown.ends_with('\u{2026}'));
    }
}
