use super::rotated_text::{render_rotated_text, truncate_with_ellipsis};
use super::widgets::{icon_text, panel_trigger};
use crate::media::{MediaState, Trigger};
use crate::panel::PanelKind;
use crate::services::media::Player;
use crate::Message;
use ab_glyph::FontArc;
use iced::widget::{column, image, lazy};
use iced::{Alignment, Element};
use obayebar::style;

/// Longest label the bar shows before truncating it.
const MAX_LABEL_CHARS: usize = 28;

/// "title — artist" for the bar; the title alone without artists, and the
/// player's identity while its track has no title.
pub fn label(player: &Player) -> String {
    let full = player.track.title.as_ref().map_or_else(
        || player.identity.clone(),
        |title| {
            player.track.artists_line().map_or_else(
                || title.clone(),
                |artists| format!("{title} \u{2014} {artists}"),
            )
        },
    );
    truncate_with_ellipsis(&full, MAX_LABEL_CHARS)
}

/// The media bar entry, keyed so it only rebuilds when what it shows changes.
/// `None` when the entry should not show at all.
pub fn entry<'a>(
    media: Option<&MediaState>,
    font: Option<FontArc>,
    has_font: bool,
    monitor: Option<&str>,
) -> Option<Element<'a, Message>> {
    let label = match media?.trigger()? {
        Trigger::Idle => None,
        Trigger::Player(player) => Some(label(player)),
    };
    let monitor = monitor.map(String::from);
    let key = (label.clone(), has_font, monitor.clone());
    Some(
        lazy(key, move |_| {
            view(label.as_deref(), font.as_ref(), monitor.clone())
        })
        .into(),
    )
}

/// Render the bar entry: a note icon over the rotated track label, or the icon
/// alone without a `label` or a vector font, since horizontal text cannot
/// fit the bar's width. Hovering or clicking opens the media panel.
fn view<'a>(
    label: Option<&str>,
    font: Option<&FontArc>,
    monitor: Option<String>,
) -> Element<'a, Message> {
    let icon = icon_text(
        style::ICON_MUSIC_NOTE,
        style::FONT_SIZE_LARGE,
        style::M3_PRIMARY,
    );
    let rotated = font.zip(label).map(|(font, label)| {
        image(render_rotated_text(
            font,
            label,
            style::FONT_SIZE_NORMAL,
            style::M3_ON_SURFACE_VARIANT,
        ))
        .content_fit(iced::ContentFit::None)
    });

    let stack = column![icon, rotated]
        .spacing(style::SPACING_SMALL)
        .align_x(Alignment::Center);

    panel_trigger(PanelKind::Media, monitor, stack)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::media::{PlaybackStatus, Track};

    fn player(title: &str, artists: &[&str], identity: &str) -> Player {
        Player {
            track: Track {
                title: (!title.is_empty()).then(|| title.to_string()),
                artists: artists.iter().map(ToString::to_string).collect(),
                ..Track::default()
            },
            identity: identity.to_string(),
            ..Player::test_player("org.mpris.MediaPlayer2.test", PlaybackStatus::Playing)
        }
    }

    #[test]
    fn title_and_artists_are_joined_with_a_dash() {
        assert_eq!(
            label(&player("Song", &["A", "B"], "mpv")),
            "Song \u{2014} A, B"
        );
    }

    #[test]
    fn a_track_without_artists_shows_its_title() {
        assert_eq!(label(&player("Song", &[], "mpv")), "Song");
    }

    #[test]
    fn a_track_without_title_shows_the_player() {
        assert_eq!(label(&player("", &["A"], "Spotify")), "Spotify");
    }

    #[test]
    fn long_labels_are_truncated() {
        let long = "x".repeat(MAX_LABEL_CHARS * 2);
        let shown = label(&player(&long, &["A"], "mpv"));
        assert_eq!(shown.chars().count(), MAX_LABEL_CHARS);
        assert!(shown.ends_with('\u{2026}'));
    }
}
