use std::f32::consts::TAU;
use std::time::{Duration, Instant};

use super::rotated_text::truncate_with_ellipsis;
use super::widgets::panel_with_exit;
use crate::media::{Action, MediaState};
use crate::panel::PanelKind;
use crate::services::media::{Capability, LoopStatus, PlaybackStatus, Player};
use crate::Message;
use iced::widget::canvas::{self, Frame, Geometry, LineCap, Path, Stroke};
use iced::widget::{button, column, container, image, row, text, Space, Stack};
use iced::{mouse, Alignment, Color, Element, Length, Point, Rectangle, Renderer, Theme};
use obayebar::style;
use zbus::zvariant::OwnedObjectPath;

/// Horizontal distance between two crests of the elapsed wave.
const WAVELENGTH: f32 = 24.0;
/// Horizontal resolution of the wave polyline.
const WAVE_STEP: f32 = 2.0;
/// Time for one crest to travel a wavelength while playing.
const WAVE_PERIOD: Duration = Duration::from_millis(1600);
/// How long the wave takes to swell on play and to flatten on pause.
const EASE_DURATION: Duration = Duration::from_millis(350);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayPause {
    Play,
    Pause,
}

/// What seeking needs: `SetPosition` is addressed to a track id, and the
/// slider maps onto a known length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeekTarget {
    pub track: OwnedObjectPath,
    pub length: Duration,
}

/// The controls the panel shows. A control the player does not support is
/// absent rather than disabled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Controls {
    pub previous: bool,
    pub next: bool,
    pub play_pause: Option<PlayPause>,
    pub loop_status: Option<LoopStatus>,
    pub shuffle: Option<bool>,
    pub seek: Option<SeekTarget>,
}

impl Controls {
    const HIDDEN: Self = Self {
        previous: false,
        next: false,
        play_pause: None,
        loop_status: None,
        shuffle: None,
        seek: None,
    };

    /// A player reporting `CanControl` false accepts no command at all, so
    /// it shows none.
    fn for_player(player: &Player) -> Self {
        let caps = player.capabilities;
        if !caps.has(Capability::Control) {
            return Self::HIDDEN;
        }
        let playing = player.status == PlaybackStatus::Playing;
        Self {
            previous: caps.has(Capability::GoPrevious),
            next: caps.has(Capability::GoNext),
            play_pause: (caps.has(Capability::Play) || caps.has(Capability::Pause)).then_some(
                if playing {
                    PlayPause::Pause
                } else {
                    PlayPause::Play
                },
            ),
            loop_status: player.loop_status,
            shuffle: player.shuffle,
            seek: caps
                .has(Capability::Seek)
                .then(|| player.track.id.clone().zip(player.track.length))
                .flatten()
                .map(|(track, length)| SeekTarget { track, length }),
        }
    }
}

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

/// How far through the track `position` is, in `0.0..=1.0`.
fn progress(position: Duration, length: Option<Duration>) -> f32 {
    length
        .filter(|length| !length.is_zero())
        .map_or(0.0, |length| position.div_duration_f32(length).min(1.0))
}

/// Where along a slider of `width` the pointer at `x` points, in `0.0..=1.0`.
fn fraction_at(x: f32, width: f32) -> f32 {
    if width > 0.0 {
        (x / width).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Phase of the travelling wave after it has been animating for `elapsed`.
#[must_use]
pub fn wave_phase(elapsed: Duration) -> f32 {
    let into_period = elapsed
        .as_nanos()
        .checked_rem(WAVE_PERIOD.as_nanos())
        .and_then(|nanos| u64::try_from(nanos).ok())
        .map_or(Duration::ZERO, Duration::from_nanos);
    (into_period.div_duration_f32(WAVE_PERIOD) * TAU).rem_euclid(TAU)
}

/// The elapsed part of the slider: a sine of `amplitude` around `mid_y`
/// sampled from `start` to `end`, both ends included.
fn wave_points(start: f32, end: f32, mid_y: f32, amplitude: f32, phase: f32) -> Vec<Point> {
    let y = |x: f32| amplitude.mul_add((x / WAVELENGTH).mul_add(TAU, -phase).sin(), mid_y);
    std::iter::successors(Some(start), |x| {
        Some(x + WAVE_STEP).filter(|next| *next < end)
    })
    .chain((end > start).then_some(end))
    .map(|x| Point::new(x, y(x)))
    .collect()
}

/// The wave's amplitude as a fraction of its full height, eased between flat
/// (paused) and full (playing).
#[derive(Debug, Clone, Copy, Default)]
pub struct WaveEase {
    from: f32,
    playing: bool,
    started: Option<Instant>,
}

impl WaveEase {
    const fn goal(self) -> f32 {
        if self.playing {
            1.0
        } else {
            0.0
        }
    }

    #[must_use]
    pub fn level(&self, now: Instant) -> f32 {
        let Some(started) = self.started else {
            return self.goal();
        };
        let t = now
            .saturating_duration_since(started)
            .div_duration_f32(EASE_DURATION)
            .min(1.0);
        let eased = t * t * 2.0f32.mul_add(-t, 3.0);
        (self.goal() - self.from).mul_add(eased, self.from)
    }

    /// Head for full amplitude when `playing`, flat otherwise, starting from
    /// wherever the wave is at `now`.
    pub fn target(&mut self, playing: bool, now: Instant) {
        if playing == self.playing {
            return;
        }
        self.from = self.level(now);
        self.playing = playing;
        self.started = Some(now);
    }

    #[must_use]
    pub fn is_moving(&self, now: Instant) -> bool {
        self.started
            .is_some_and(|started| now.saturating_duration_since(started) < EASE_DURATION)
    }
}

/// Height of the slider canvas, and room for the wave's crests.
const SLIDER_HEIGHT: f32 = 28.0;
/// Peak deviation of the wave from the track line at full amplitude.
const WAVE_AMPLITUDE: f32 = 3.5;
const TRACK_WIDTH: f32 = 3.0;
const THUMB_RADIUS: f32 = 6.0;
const PLAY_BUTTON_SIZE: f32 = 52.0;
const MAX_TITLE_CHARS: usize = 26;
const MAX_ARTIST_CHARS: usize = 36;
/// Darkening laid over the cover so text stays legible on any art.
const SCRIM_ALPHA: f32 = 0.55;

#[derive(Debug, Default)]
pub struct SliderState {
    dragging: Option<f32>,
}

/// The Android-style progress slider: a travelling wave up to the thumb,
/// a straight line after it. Seeking is offered only with a [`SeekTarget`];
/// without one the slider only displays.
#[derive(Debug)]
struct WaveSlider {
    progress: f32,
    amplitude: f32,
    phase: f32,
    seek_length: Option<Duration>,
}

impl WaveSlider {
    fn fraction(bounds: Rectangle, x: f32) -> f32 {
        fraction_at(
            x - bounds.x - THUMB_RADIUS,
            THUMB_RADIUS.mul_add(-2.0, bounds.width),
        )
    }
}

impl canvas::Program<Message> for WaveSlider {
    type State = SliderState;

    fn update(
        &self,
        state: &mut Self::State,
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let length = self.seek_length?;
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let position = cursor.position_over(bounds)?;
                state.dragging = Some(Self::fraction(bounds, position.x));
                Some(canvas::Action::request_redraw().and_capture())
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                state.dragging.as_mut().map(|dragging| {
                    *dragging = Self::fraction(bounds, position.x);
                    canvas::Action::request_redraw()
                })
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let fraction = state.dragging.take()?;
                Some(
                    canvas::Action::publish(Message::MediaControl(Action::Seek(
                        length.mul_f32(fraction),
                    )))
                    .and_capture(),
                )
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry<Renderer>> {
        let mut frame = Frame::new(renderer, bounds.size());
        let mid = bounds.height / 2.0;
        let (start, end) = (THUMB_RADIUS, bounds.width - THUMB_RADIUS);
        let thumb_x = (end - start).mul_add(state.dragging.unwrap_or(self.progress), start);

        let wave = Path::new(|builder| {
            let mut points =
                wave_points(start, thumb_x, mid, self.amplitude, self.phase).into_iter();
            if let Some(first) = points.next() {
                builder.move_to(first);
                points.for_each(|point| builder.line_to(point));
            }
        });
        let stroke = |color| {
            Stroke::default()
                .with_width(TRACK_WIDTH)
                .with_color(color)
                .with_line_cap(LineCap::Round)
        };
        frame.stroke(&wave, stroke(style::M3_PRIMARY));
        if thumb_x < end {
            frame.stroke(
                &Path::line(Point::new(thumb_x, mid), Point::new(end, mid)),
                stroke(style::with_alpha(style::M3_ON_SURFACE, 0.3)),
            );
        }
        frame.fill(
            &Path::circle(Point::new(thumb_x, mid), THUMB_RADIUS),
            style::M3_PRIMARY,
        );
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.dragging.is_some() {
            mouse::Interaction::Grabbing
        } else if self.seek_length.is_some() && cursor.is_over(bounds) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }
}

fn icon(glyph: &str, size: f32, color: Color) -> iced::widget::Text<'_> {
    text(glyph)
        .font(style::ICON_FONT)
        .size(size)
        .color(color)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
}

fn control(glyph: &str, color: Color, action: Action) -> Element<'_, Message> {
    button(icon(glyph, style::FONT_SIZE_LARGE, color))
        .on_press(Message::MediaControl(action))
        .style(style::hover_button(
            Color::TRANSPARENT,
            color,
            style::ROUNDING_FULL,
        ))
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

/// The player's name; a button cycling to the next player when there is one.
fn player_chip(identity: &str, cycles: bool) -> Element<'_, Message> {
    let label = text(identity)
        .size(style::FONT_SIZE_SMALL)
        .color(style::M3_ON_SURFACE);
    let bg = style::with_alpha(style::M3_SURFACE_CONTAINER_HIGHEST, 0.7);
    let chip = button(label)
        .padding([2.0, style::PADDING_NORMAL])
        .style(style::hover_button(
            bg,
            style::M3_ON_SURFACE,
            style::ROUNDING_FULL,
        ));
    if cycles {
        chip.on_press(Message::MediaCyclePlayer).into()
    } else {
        chip.into()
    }
}

fn play_button(state: PlayPause) -> Element<'static, Message> {
    let glyph = match state {
        PlayPause::Play => style::ICON_PLAY_ARROW,
        PlayPause::Pause => style::ICON_PAUSE,
    };
    button(
        icon(glyph, style::FONT_SIZE_EXTRA_LARGE, style::M3_ON_PRIMARY)
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .on_press(Message::MediaControl(Action::PlayPause))
    .width(PLAY_BUTTON_SIZE)
    .height(PLAY_BUTTON_SIZE)
    .padding(0)
    .style(style::hover_button(
        style::M3_PRIMARY,
        style::M3_ON_PRIMARY,
        style::ROUNDING_FULL,
    ))
    .into()
}

fn header<'a>(media: &'a MediaState, player: &'a Player) -> Element<'a, Message> {
    let elapsed = player.position_at(media.now());
    let times = player.track.length.map_or_else(
        || format_time(elapsed),
        |length| format!("{} / {}", format_time(elapsed), format_time(length)),
    );
    row![
        icon(
            style::ICON_MUSIC_NOTE,
            style::FONT_SIZE_NORMAL,
            style::M3_PRIMARY
        ),
        text(times)
            .size(style::FONT_SIZE_SMALL)
            .color(style::M3_ON_SURFACE_VARIANT),
        Space::new().width(Length::Fill),
        player_chip(&player.identity, media.players().len() > 1),
    ]
    .spacing(style::SPACING_SMALL)
    .align_y(Alignment::Center)
    .into()
}

fn now_playing(player: &Player, play_pause: Option<PlayPause>) -> Element<'_, Message> {
    let title = if player.track.title.is_empty() {
        &player.identity
    } else {
        &player.track.title
    };
    let mut labels = column![text(truncate_with_ellipsis(title, MAX_TITLE_CHARS))
        .size(style::FONT_SIZE_LARGE)
        .color(style::M3_ON_SURFACE)]
    .spacing(2.0)
    .width(Length::Fill);
    if !player.track.artists.is_empty() {
        labels = labels.push(
            text(truncate_with_ellipsis(
                &player.track.artists.join(", "),
                MAX_ARTIST_CHARS,
            ))
            .size(style::FONT_SIZE_NORMAL)
            .color(style::M3_ON_SURFACE_VARIANT),
        );
    }
    let mut body = row![labels].align_y(Alignment::Center);
    if let Some(state) = play_pause {
        body = body.push(play_button(state));
    }
    body.into()
}

fn transport<'a>(media: &MediaState, player: &Player, controls: &Controls) -> Element<'a, Message> {
    let slider = WaveSlider {
        progress: progress(player.position_at(media.now()), player.track.length),
        amplitude: WAVE_AMPLITUDE * media.wave_level(),
        phase: media.wave_phase(),
        seek_length: controls.seek.as_ref().map(|seek| seek.length),
    };
    let mut bar = row![].spacing(2.0).align_y(Alignment::Center);
    if controls.previous {
        bar = bar.push(control(
            style::ICON_SKIP_PREVIOUS,
            style::M3_ON_SURFACE,
            Action::Previous,
        ));
    }
    bar = bar.push(
        canvas::Canvas::new(slider)
            .width(Length::Fill)
            .height(Length::Fixed(SLIDER_HEIGHT)),
    );
    if controls.next {
        bar = bar.push(control(
            style::ICON_SKIP_NEXT,
            style::M3_ON_SURFACE,
            Action::Next,
        ));
    }
    if let Some(status) = controls.loop_status {
        bar = bar.push(control(
            loop_icon(status),
            toggle_color(status != LoopStatus::None),
            Action::CycleLoop,
        ));
    }
    if let Some(shuffle) = controls.shuffle {
        bar = bar.push(control(
            style::ICON_SHUFFLE,
            toggle_color(shuffle),
            Action::ToggleShuffle,
        ));
    }
    bar.into()
}

/// The cover filling the card under a scrim, or the plain panel background.
fn background(art: Option<&image::Handle>) -> Element<'_, Message> {
    let rounded = |alpha| {
        move |_theme: &Theme| container::Style {
            background: Some(iced::Background::Color(style::with_alpha(
                Color::BLACK,
                alpha,
            ))),
            border: iced::Border {
                radius: style::ROUNDING_NORMAL.into(),
                ..iced::Border::default()
            },
            ..container::Style::default()
        }
    };
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
                container(fill()).style(rounded(SCRIM_ALPHA)).into(),
            ])
            .into()
        },
    )
}

fn card_content<'a>(media: &'a MediaState, player: &'a Player) -> Element<'a, Message> {
    let controls = Controls::for_player(player);
    column![
        header(media, player),
        now_playing(player, controls.play_pause),
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
    use crate::services::media::{Capabilities, PositionSample, Track};

    const ALL: [Capability; 6] = [
        Capability::GoNext,
        Capability::GoPrevious,
        Capability::Play,
        Capability::Pause,
        Capability::Seek,
        Capability::Control,
    ];

    fn all_capabilities() -> Capabilities {
        without(&[])
    }

    fn track_id() -> OwnedObjectPath {
        OwnedObjectPath::try_from("/track/1").unwrap_or_else(|e| unreachable!("{e}"))
    }

    fn player(capabilities: Capabilities) -> Player {
        Player {
            bus_name: "org.mpris.MediaPlayer2.test".to_string(),
            identity: "Test".to_string(),
            status: PlaybackStatus::Playing,
            track: Track {
                id: Some(track_id()),
                length: Some(Duration::from_secs(200)),
                ..Track::default()
            },
            position: PositionSample {
                position: Duration::ZERO,
                sampled_at: Instant::now(),
                rate: 1.0,
            },
            capabilities,
            loop_status: Some(LoopStatus::Track),
            shuffle: Some(false),
        }
    }

    fn without(missing: &[Capability]) -> Capabilities {
        ALL.into_iter()
            .filter(|c| !missing.contains(c))
            .fold(Capabilities::default(), Capabilities::with)
    }

    #[test]
    fn a_fully_capable_player_shows_every_control() {
        let controls = Controls::for_player(&player(all_capabilities()));
        assert_eq!(
            controls,
            Controls {
                previous: true,
                next: true,
                play_pause: Some(PlayPause::Pause),
                loop_status: Some(LoopStatus::Track),
                shuffle: Some(false),
                seek: Some(SeekTarget {
                    track: track_id(),
                    length: Duration::from_secs(200),
                }),
            }
        );
    }

    #[test]
    fn unsupported_skips_are_hidden() {
        let controls = Controls::for_player(&player(without(&[Capability::GoPrevious])));
        assert!(!controls.previous);
        assert!(controls.next);
        let controls = Controls::for_player(&player(without(&[Capability::GoNext])));
        assert!(controls.previous);
        assert!(!controls.next);
    }

    #[test]
    fn play_pause_needs_either_capability() {
        let neither = without(&[Capability::Play, Capability::Pause]);
        assert_eq!(Controls::for_player(&player(neither)).play_pause, None);

        let pause_only = Capabilities::default()
            .with(Capability::Control)
            .with(Capability::Pause);
        assert_eq!(
            Controls::for_player(&player(pause_only)).play_pause,
            Some(PlayPause::Pause)
        );

        let mut paused = player(
            Capabilities::default()
                .with(Capability::Control)
                .with(Capability::Play),
        );
        paused.status = PlaybackStatus::Paused;
        assert_eq!(
            Controls::for_player(&paused).play_pause,
            Some(PlayPause::Play)
        );
    }

    #[test]
    fn absent_loop_and_shuffle_are_hidden() {
        let mut p = player(all_capabilities());
        p.loop_status = None;
        p.shuffle = None;
        let controls = Controls::for_player(&p);
        assert_eq!(controls.loop_status, None);
        assert_eq!(controls.shuffle, None);
    }

    #[test]
    fn a_player_that_cannot_be_controlled_shows_nothing() {
        let controls = Controls::for_player(&player(without(&[Capability::Control])));
        assert_eq!(
            controls,
            Controls {
                previous: false,
                next: false,
                play_pause: None,
                loop_status: None,
                shuffle: None,
                seek: None,
            }
        );
    }

    #[test]
    fn seeking_needs_the_capability_a_track_id_and_a_length() {
        assert_eq!(
            Controls::for_player(&player(without(&[Capability::Seek]))).seek,
            None
        );
        let mut no_id = player(all_capabilities());
        no_id.track.id = None;
        assert_eq!(Controls::for_player(&no_id).seek, None);
        let mut no_length = player(all_capabilities());
        no_length.track.length = None;
        assert_eq!(Controls::for_player(&no_length).seek, None);
    }

    #[test]
    fn times_are_formatted_like_a_player_does() {
        assert_eq!(format_time(Duration::ZERO), "0:00");
        assert_eq!(format_time(Duration::from_millis(187_900)), "3:07");
        assert_eq!(format_time(Duration::from_secs(3723)), "1:02:03");
    }

    #[test]
    fn progress_is_a_clamped_fraction() {
        let length = Some(Duration::from_secs(100));
        assert!((progress(Duration::from_secs(25), length) - 0.25).abs() < f32::EPSILON);
        assert!((progress(Duration::from_secs(250), length) - 1.0).abs() < f32::EPSILON);
        assert!(progress(Duration::from_secs(25), None).abs() < f32::EPSILON);
        assert!(progress(Duration::from_secs(25), Some(Duration::ZERO)).abs() < f32::EPSILON);
    }

    #[test]
    fn pointer_fraction_is_clamped_to_the_slider() {
        assert!((fraction_at(50.0, 200.0) - 0.25).abs() < f32::EPSILON);
        assert!(fraction_at(-10.0, 200.0).abs() < f32::EPSILON);
        assert!((fraction_at(900.0, 200.0) - 1.0).abs() < f32::EPSILON);
        assert!(fraction_at(10.0, 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn the_phase_stays_within_one_turn() {
        for secs in [0, 1, 7, 1000] {
            let phase = wave_phase(Duration::from_secs(secs));
            assert!((0.0..std::f32::consts::TAU).contains(&phase), "{phase}");
        }
        assert!(wave_phase(Duration::from_millis(100)) > 0.0);
    }

    #[test]
    fn the_wave_spans_its_range_within_its_amplitude() {
        let points = wave_points(10.0, 100.0, 20.0, 4.0, 1.0);
        assert!(points.len() > 2);
        assert_eq!(points.first().map(|p| p.x), Some(10.0));
        assert_eq!(points.last().map(|p| p.x), Some(100.0));
        assert!(points
            .iter()
            .all(|p| (p.y - 20.0).abs() <= 4.0 + f32::EPSILON));
        assert!(points.iter().any(|p| (p.y - 20.0).abs() > 1.0));
        assert!(points.windows(2).all(|w| matches!(w, [a, b] if a.x < b.x)));
    }

    #[test]
    fn a_zero_amplitude_wave_is_a_straight_line() {
        let points = wave_points(0.0, 50.0, 8.0, 0.0, 2.0);
        assert!(points.iter().all(|p| (p.y - 8.0).abs() < f32::EPSILON));
    }

    #[test]
    fn an_empty_range_is_a_single_point() {
        assert_eq!(wave_points(30.0, 30.0, 8.0, 0.0, 0.0).len(), 1);
        assert_eq!(wave_points(30.0, 10.0, 8.0, 0.0, 0.0).len(), 1);
    }

    #[test]
    fn a_fresh_wave_is_flat_and_still() {
        let now = Instant::now();
        let ease = WaveEase::default();
        assert!(ease.level(now).abs() < f32::EPSILON);
        assert!(!ease.is_moving(now));
    }

    #[test]
    fn playing_swells_the_wave_over_the_ease() {
        let t0 = Instant::now();
        let mut ease = WaveEase::default();
        ease.target(true, t0);
        let mid = t0 + EASE_DURATION / 2;
        assert!(ease.level(t0).abs() < f32::EPSILON);
        assert!(ease.level(mid) > 0.0 && ease.level(mid) < 1.0);
        assert!(ease.is_moving(mid));
        let done = t0 + EASE_DURATION;
        assert!((ease.level(done) - 1.0).abs() < f32::EPSILON);
        assert!(!ease.is_moving(done));
    }

    #[test]
    fn retargeting_midway_continues_from_the_current_level() {
        let t0 = Instant::now();
        let mut ease = WaveEase::default();
        ease.target(true, t0);
        let mid = t0 + EASE_DURATION / 2;
        let before = ease.level(mid);
        ease.target(false, mid);
        assert!((ease.level(mid) - before).abs() < f32::EPSILON);
        assert!(ease.level(mid + EASE_DURATION).abs() < f32::EPSILON);
    }

    #[test]
    fn retargeting_the_same_way_does_not_restart() {
        let t0 = Instant::now();
        let mut ease = WaveEase::default();
        ease.target(true, t0);
        let done = t0 + EASE_DURATION;
        ease.target(true, done);
        assert!(!ease.is_moving(done));
        assert!((ease.level(done) - 1.0).abs() < f32::EPSILON);
    }
}
