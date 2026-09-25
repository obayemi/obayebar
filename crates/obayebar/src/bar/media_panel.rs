use std::time::Duration;

use super::rotated_text::truncate_with_ellipsis;
use super::wave_slider;
use super::widgets::{icon_text, panel_with_exit};
use crate::media::controls::{Controls, PlayPause};
use crate::media::{Action, MediaState, Message as MediaMessage, Rotation};
use crate::panel::PanelKind;
use crate::services::media::{LoopStatus, Player};
use crate::Message;
use iced::widget::{button, column, container, image, row, text, Space, Stack};
use iced::{Alignment, Color, Element, Length, Theme};
use obayebar::style;

/// Peak deviation of the wave from the track line at full amplitude.
const WAVE_AMPLITUDE: f32 = 3.5;
const PLAY_BUTTON_SIZE: f32 = 52.0;
const MAX_TITLE_CHARS: usize = 26;
const MAX_ARTIST_CHARS: usize = 36;
/// Darkening laid over the cover so text stays legible on any art.
const SCRIM_ALPHA: f32 = 0.55;
/// Extra darkening under the text laid over the cover.
const SURFACE_ALPHA: f32 = 0.4;

/// `m:ss`, or `h:mm:ss` from an hour up.
fn format_time(time: Duration) -> String {
    let secs = time.as_secs();
    let (hours, minutes, seconds) = (secs / 3600, secs / 60 % 60, secs % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Panel flavour of `style::hover_button`, pinned to the fully-round corner
/// radius the transport and player-chip buttons share.
fn round_button(bg: Color, text_color: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    style::hover_button(bg, text_color, style::ROUNDING_FULL)
}

fn control(glyph: &str, color: Color, action: Action) -> Element<'_, Message> {
    button(icon_text(glyph, style::FONT_SIZE_LARGE, color))
        .on_press(Message::Media(MediaMessage::Control(action)))
        .style(round_button(Color::TRANSPARENT, color))
        .padding(style::PADDING_SMALL)
        .into()
}

const fn toggle_color(on: bool) -> Color {
    if on {
        style::M3_PRIMARY
    } else {
        style::M3_ON_SURFACE_VARIANT
    }
}

const fn loop_icon(status: LoopStatus) -> &'static str {
    match status {
        LoopStatus::Track => style::ICON_REPEAT_ONE,
        LoopStatus::None | LoopStatus::Playlist => style::ICON_REPEAT,
    }
}

/// The player's name, followed by its place among the others when there are.
fn chip_label(identity: &str, rotation: Option<Rotation>) -> String {
    rotation.map_or_else(
        || identity.to_string(),
        |Rotation { index, total }| format!("{identity} · {index}/{total}"),
    )
}

/// The player's name; a button cycling to the next player when there is one.
fn player_chip(identity: &str, rotation: Option<Rotation>) -> Element<'_, Message> {
    let label = row![
        rotation.map(|_| icon_text(
            style::ICON_SWAP_HORIZ,
            style::FONT_SIZE_SMALL,
            style::M3_PRIMARY
        )),
        text(chip_label(identity, rotation))
            .size(style::FONT_SIZE_SMALL)
            .color(style::M3_ON_SURFACE),
    ]
    .spacing(style::SPACING_SMALL)
    .align_y(Alignment::Center);
    let bg = style::with_alpha(style::M3_SURFACE_CONTAINER_HIGHEST, 0.7);
    button(label)
        .padding([2.0, style::PADDING_NORMAL])
        .style(round_button(bg, style::M3_ON_SURFACE))
        .on_press_maybe(rotation.map(|_| Message::Media(MediaMessage::CyclePlayer)))
        .into()
}

/// Lay a darker surface under `content` while it sits over a cover, so it
/// stays legible on light art. The padding holds either way so nothing
/// shifts when a cover loads.
fn legible<'a>(content: impl Into<Element<'a, Message>>, over_art: bool) -> Element<'a, Message> {
    let surface = move |_: &Theme| container::Style {
        background: over_art
            .then(|| iced::Background::Color(style::with_alpha(Color::BLACK, SURFACE_ALPHA))),
        border: iced::Border {
            radius: style::ROUNDING_SMALL.into(),
            ..iced::Border::default()
        },
        ..container::Style::default()
    };
    container(content)
        .padding([2.0, style::PADDING_SMALL])
        .style(surface)
        .into()
}

fn play_button(state: PlayPause) -> Element<'static, Message> {
    let glyph = match state {
        PlayPause::Play => style::ICON_PLAY_ARROW,
        PlayPause::Pause => style::ICON_PAUSE,
    };
    button(
        icon_text(glyph, style::FONT_SIZE_EXTRA_LARGE, style::M3_ON_PRIMARY)
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .on_press(Message::Media(MediaMessage::Control(Action::PlayPause)))
    .width(PLAY_BUTTON_SIZE)
    .height(PLAY_BUTTON_SIZE)
    .padding(0)
    .style(round_button(style::M3_PRIMARY, style::M3_ON_PRIMARY))
    .into()
}

fn header<'a>(media: &'a MediaState, player: &'a Player, over_art: bool) -> Element<'a, Message> {
    let elapsed = player.position_at(media.now());
    let times = player.track.length.map_or_else(
        || format_time(elapsed),
        |length| format!("{} / {}", format_time(elapsed), format_time(length)),
    );
    let clock = row![
        icon_text(
            style::ICON_MUSIC_NOTE,
            style::FONT_SIZE_NORMAL,
            style::M3_PRIMARY
        ),
        text(times)
            .size(style::FONT_SIZE_SMALL)
            .color(style::M3_ON_SURFACE_VARIANT),
    ]
    .spacing(style::SPACING_SMALL)
    .align_y(Alignment::Center);
    row![
        legible(clock, over_art),
        Space::new().width(Length::Fill),
        player_chip(&player.identity, media.rotation()),
    ]
    .align_y(Alignment::Center)
    .into()
}

fn now_playing(
    player: &Player,
    play_pause: Option<PlayPause>,
    over_art: bool,
) -> Element<'_, Message> {
    let labels = column![
        text(truncate_with_ellipsis(player.title(), MAX_TITLE_CHARS))
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_ON_SURFACE),
        player.track.artists_line().map(|artists| {
            text(truncate_with_ellipsis(&artists, MAX_ARTIST_CHARS))
                .size(style::FONT_SIZE_NORMAL)
                .color(style::M3_ON_SURFACE_VARIANT)
        }),
    ]
    .spacing(2.0);
    row![
        legible(labels, over_art),
        Space::new().width(Length::Fill),
        play_pause.map(play_button),
    ]
    .align_y(Alignment::Center)
    .into()
}

fn transport<'a>(media: &MediaState, player: &Player, controls: &Controls) -> Element<'a, Message> {
    let slider = wave_slider::view(
        wave_slider::progress(player.position_at(media.now()), player.track.length),
        WAVE_AMPLITUDE * media.wave_level(),
        media.wave_phase(),
        controls.seek.as_ref().map(|seek| seek.length),
    );
    row![
        controls.previous.then(|| control(
            style::ICON_SKIP_PREVIOUS,
            style::M3_ON_SURFACE,
            Action::Previous
        )),
        slider,
        controls
            .next
            .then(|| control(style::ICON_SKIP_NEXT, style::M3_ON_SURFACE, Action::Next)),
        controls.loop_status.map(|status| control(
            loop_icon(status),
            toggle_color(status != LoopStatus::None),
            Action::CycleLoop,
        )),
        controls.shuffle.map(|shuffle| control(
            style::ICON_SHUFFLE,
            toggle_color(shuffle),
            Action::ToggleShuffle,
        )),
    ]
    .spacing(2.0)
    .align_y(Alignment::Center)
    .into()
}

/// Darken the cover so the overlaid text stays legible on any art.
fn scrim(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(style::with_alpha(
            Color::BLACK,
            SCRIM_ALPHA,
        ))),
        border: iced::Border {
            radius: style::ROUNDING_NORMAL.into(),
            ..iced::Border::default()
        },
        ..container::Style::default()
    }
}

/// The cover filling the card under a scrim, or the plain panel background.
fn background(art: Option<&image::Handle>) -> Element<'_, Message> {
    let fill = || Space::new().width(Length::Fill).height(Length::Fill);
    art.map_or_else(
        || container(fill()).style(style::panel_container).into(),
        |handle| {
            Stack::with_children(vec![
                image(handle.clone())
                    .content_fit(iced::ContentFit::Cover)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .border_radius(style::ROUNDING_NORMAL)
                    .into(),
                container(fill()).style(scrim).into(),
            ])
            .into()
        },
    )
}

fn card_content<'a>(media: &'a MediaState, player: &'a Player) -> Element<'a, Message> {
    let controls = Controls::for_player(player);
    let over_art = media.art().is_some();
    column![
        header(media, player, over_art),
        now_playing(player, controls.play_pause, over_art),
        Space::new().height(Length::Fill),
        transport(media, player, &controls),
    ]
    .spacing(style::SPACING_NORMAL)
    .into()
}

pub fn view(media: &MediaState) -> Element<'_, Message> {
    let content: Element<'_, Message> = media.active().map_or_else(
        || {
            text("Nothing playing")
                .size(style::FONT_SIZE_NORMAL)
                .color(style::M3_ON_SURFACE_VARIANT)
                .into()
        },
        |player| card_content(media, player),
    );
    let card = Stack::with_children(vec![
        background(media.art()),
        container(content)
            .padding(style::PADDING_LARGE)
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
    ]);
    let panel = container(card)
        .width(Length::Fill)
        .height(Length::Fixed(f32::from(style::MEDIA_PANEL_HEIGHT)));

    panel_with_exit(PanelKind::Media, panel.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_formatted_like_a_player_does() {
        assert_eq!(format_time(Duration::ZERO), "0:00");
        assert_eq!(format_time(Duration::from_millis(187_900)), "3:07");
        assert_eq!(format_time(Duration::from_secs(3723)), "1:02:03");
    }

    #[test]
    fn the_chip_counts_players_only_when_there_are_others() {
        assert_eq!(chip_label("Spotify", None), "Spotify");
        assert_eq!(
            chip_label("Spotify", Some(Rotation { index: 1, total: 3 })),
            "Spotify · 1/3"
        );
    }
}
