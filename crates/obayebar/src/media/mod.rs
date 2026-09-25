//! Everything the bar keeps about media playback between messages: the
//! players the service reported, which one is shown, their covers and the
//! slider's wave.

mod art_cache;
pub mod controls;
pub mod selection;

use std::f32::consts::TAU;
use std::time::{Duration, Instant};

use iced::widget::image;
use iced::Animation;

use art_cache::ArtCache;
pub use controls::Controls;
use selection::Selection;

use crate::services::media::{Command, Player};
use crate::services::media_art::Art;

/// Time for one crest of the elapsed wave to travel a wavelength while
/// playing.
const WAVE_PERIOD: Duration = Duration::from_millis(1600);
/// How long the wave takes to swell on play and to flatten on pause.
const EASE_DURATION: Duration = Duration::from_millis(350);

/// What the bar shows for media.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Trigger<'a> {
    /// The icon alone: no player to name.
    Idle,
    Player(&'a Player),
}

/// What the panel asks of the active player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    PlayPause,
    Next,
    Previous,
    CycleLoop,
    ToggleShuffle,
    Seek(Duration),
}

/// A message from the media service or the panel's widgets.
#[derive(Debug, Clone)]
pub enum Message {
    /// A fresh snapshot of every MPRIS player.
    Players(Vec<Player>),
    /// A cover fetch for this URL finished.
    Art(String, Art),
    Control(Action),
    CyclePlayer,
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

#[derive(Debug)]
pub struct MediaState {
    players: Vec<Player>,
    selection: Selection,
    art: ArtCache,
    wave: Animation<bool>,
    /// When the state was created, the origin of the wave's phase.
    epoch: Instant,
    /// The clock as of the last message, so views stay pure.
    now: Instant,
    /// `[media].show_when_idle`: keep the bar entry while nothing plays.
    show_when_idle: bool,
}

impl MediaState {
    #[must_use]
    pub fn new(now: Instant, show_when_idle: bool) -> Self {
        Self {
            players: Vec::new(),
            selection: Selection::default(),
            art: ArtCache::default(),
            wave: Animation::new(false).duration(EASE_DURATION),
            epoch: now,
            now,
            show_when_idle,
        }
    }

    /// Adopt a snapshot from the service. Returns a cover URL to fetch when
    /// the shown track has one that is not cached yet.
    pub fn update(&mut self, players: Vec<Player>) -> Option<String> {
        self.selection.observe(&players);
        self.players = players;
        self.follow_active()
    }

    /// Show the next player. Returns a cover URL to fetch, as [`Self::update`].
    pub fn cycle_player(&mut self) -> Option<String> {
        self.selection.cycle(&self.players);
        self.follow_active()
    }

    /// Point the wave at the active player and ask for its cover.
    fn follow_active(&mut self) -> Option<String> {
        let active = self.selection.active(&self.players);
        let playing = active.is_some_and(Player::is_playing);
        let art_url = active.and_then(|p| p.track.art_url.clone());
        self.wave.go_mut(playing, self.now);
        art_url.filter(|url| self.art.request(url))
    }

    pub fn art_loaded(&mut self, url: String, art: Art) {
        self.art.insert(url, art);
    }

    /// Send `action` to the active player, and move the slider ahead of it
    /// when it seeks. `None` when there is no active player or it does not
    /// support the action.
    pub fn apply(&mut self, action: Action) -> Option<(String, Command)> {
        let player = self.active()?;
        let controls = Controls::for_player(player);
        let bus_name = player.bus_name.clone();
        let command = match action {
            Action::PlayPause => controls.play_pause.map(|_| Command::PlayPause),
            Action::Next => controls.next.then_some(Command::Next),
            Action::Previous => controls.previous.then_some(Command::Previous),
            Action::CycleLoop => controls
                .loop_status
                .map(|status| Command::SetLoopStatus(status.next())),
            Action::ToggleShuffle => controls
                .shuffle
                .map(|shuffle| Command::SetShuffle(!shuffle)),
            Action::Seek(position) => controls.seek.as_ref().map(|seek| Command::SetPosition {
                track: seek.track.clone(),
                position,
            }),
        }?;
        if let Action::Seek(position) = action {
            self.seek(position);
        }
        Some((bus_name, command))
    }

    /// Move the slider to `position` ahead of the player confirming it.
    fn seek(&mut self, position: Duration) {
        let Some(bus) = self.active().map(|p| p.bus_name.clone()) else {
            return;
        };
        if let Some(player) = self.players.iter_mut().find(|p| p.bus_name == bus) {
            player.position.position = position;
            player.position.sampled_at = self.now;
        }
    }

    pub const fn tick(&mut self, now: Instant) {
        self.now = now;
    }

    /// The bar entry: the active player while one plays, and otherwise only
    /// when `show_when_idle` asks for it. `None` when the entry should not
    /// show at all.
    #[must_use]
    pub fn trigger(&self) -> Option<Trigger<'_>> {
        let playing = self.players.iter().any(Player::is_playing);
        match self.active() {
            Some(player) if playing || self.show_when_idle => Some(Trigger::Player(player)),
            None if self.show_when_idle => Some(Trigger::Idle),
            _ => None,
        }
    }

    #[must_use]
    pub fn active(&self) -> Option<&Player> {
        self.selection.active(&self.players)
    }

    #[must_use]
    pub fn controls(&self) -> Controls {
        self.active().map(Controls::for_player).unwrap_or_default()
    }

    #[must_use]
    pub const fn players(&self) -> &[Player] {
        self.players.as_slice()
    }

    /// The active track's cover, once it has loaded.
    #[must_use]
    pub fn art(&self) -> Option<&image::Handle> {
        let url = self.active()?.track.art_url.as_deref()?;
        match self.art.peek(url)? {
            Art::Loaded(handle) => Some(handle),
            Art::Failed => None,
        }
    }

    #[must_use]
    pub const fn now(&self) -> Instant {
        self.now
    }

    #[must_use]
    pub fn wave_level(&self) -> f32 {
        self.wave.interpolate(0.0_f32, 1.0, self.now)
    }

    #[must_use]
    pub fn wave_phase(&self) -> f32 {
        wave_phase(self.now.saturating_duration_since(self.epoch))
    }

    /// Whether the slider needs frames: while playing, and while the wave is
    /// still flattening after a pause.
    #[must_use]
    pub fn animating(&self) -> bool {
        self.active().is_some_and(Player::is_playing) || self.wave.is_animating(self.now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::media::{PlaybackStatus, Track};

    fn player(bus: &str, status: PlaybackStatus, art: Option<&str>, at: Instant) -> Player {
        Player {
            track: Track {
                art_url: art.map(ToString::to_string),
                length: Some(Duration::from_secs(100)),
                ..Track::default()
            },
            position: crate::services::media::PositionSample {
                position: Duration::from_secs(10),
                sampled_at: at,
                rate: 1.0,
            },
            ..Player::test_player(bus, status)
        }
    }

    fn loaded() -> Art {
        Art::Loaded(image::Handle::from_rgba(1, 1, vec![0; 4]))
    }

    fn bus(trigger: Option<Trigger<'_>>) -> Option<&str> {
        match trigger {
            Some(Trigger::Player(p)) => Some(p.bus_name.as_str()),
            Some(Trigger::Idle) | None => None,
        }
    }

    #[test]
    fn no_player_shows_the_idle_icon_only_when_asked() {
        let t0 = Instant::now();
        assert_eq!(MediaState::new(t0, true).trigger(), Some(Trigger::Idle));
        assert_eq!(MediaState::new(t0, false).trigger(), None);
    }

    #[test]
    fn a_paused_player_shows_only_when_asked() {
        let t0 = Instant::now();
        let paused = vec![player("a", PlaybackStatus::Paused, None, t0)];
        let mut shown = MediaState::new(t0, true);
        shown.update(paused.clone());
        assert_eq!(bus(shown.trigger()), Some("a"));
        let mut hidden = MediaState::new(t0, false);
        hidden.update(paused);
        assert_eq!(hidden.trigger(), None);
    }

    #[test]
    fn a_playing_player_always_shows() {
        let t0 = Instant::now();
        for show_when_idle in [true, false] {
            let mut media = MediaState::new(t0, show_when_idle);
            media.update(vec![
                player("a", PlaybackStatus::Stopped, None, t0),
                player("b", PlaybackStatus::Playing, None, t0),
            ]);
            assert_eq!(bus(media.trigger()), Some("b"));
        }
    }

    #[test]
    fn any_playing_player_keeps_the_entry_up() {
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, false);
        media.update(vec![
            player("a", PlaybackStatus::Playing, None, t0),
            player("b", PlaybackStatus::Paused, None, t0),
        ]);
        media.cycle_player();
        assert_eq!(bus(media.trigger()), Some("b"));
    }

    #[test]
    fn nothing_is_shown_without_players() {
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, true);
        assert_eq!(media.update(Vec::new()), None);
        assert!(media.active().is_none());
        assert!(!media.animating());
    }

    #[test]
    fn the_active_cover_is_requested_once() {
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, true);
        let players = vec![player(
            "a",
            PlaybackStatus::Paused,
            Some("file:///a.png"),
            t0,
        )];
        assert_eq!(
            media.update(players.clone()),
            Some("file:///a.png".to_string())
        );
        assert_eq!(media.update(players), None);
    }

    #[test]
    fn a_loaded_cover_is_shown_for_its_track_only() {
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, true);
        media.update(vec![player("a", PlaybackStatus::Paused, Some("u1"), t0)]);
        media.art_loaded("u1".to_string(), loaded());
        assert!(media.art().is_some());

        let next = media.update(vec![player("a", PlaybackStatus::Paused, Some("u2"), t0)]);
        assert_eq!(next, Some("u2".to_string()));
        assert!(media.art().is_none());
    }

    #[test]
    fn a_failed_cover_shows_no_art() {
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, true);
        media.update(vec![player("a", PlaybackStatus::Paused, Some("u1"), t0)]);
        media.art_loaded("u1".to_string(), Art::Failed);
        assert!(media.art().is_none());
    }

    #[test]
    fn cycling_requests_the_new_players_cover() {
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, true);
        media.update(vec![
            player("a", PlaybackStatus::Playing, Some("ua"), t0),
            player("b", PlaybackStatus::Paused, Some("ub"), t0),
        ]);
        assert_eq!(media.cycle_player(), Some("ub".to_string()));
        assert_eq!(media.active().map(|p| p.bus_name.as_str()), Some("b"));
    }

    #[test]
    fn playing_animates_and_a_pause_settles_after_the_ease() {
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, true);
        media.update(vec![player("a", PlaybackStatus::Playing, None, t0)]);
        assert!(media.animating());

        let t1 = t0 + Duration::from_secs(5);
        media.tick(t1);
        media.update(vec![player("a", PlaybackStatus::Paused, None, t1)]);
        assert!(media.animating());
        media.tick(t1 + Duration::from_secs(5));
        assert!(!media.animating());
        assert!(media.wave_level().abs() < f32::EPSILON);
    }

    fn commanded(media: &mut MediaState, action: Action) -> Option<String> {
        media
            .apply(action)
            .map(|(bus, command)| format!("{bus} {command:?}"))
    }

    #[test]
    fn no_player_takes_no_command() {
        let mut media = MediaState::new(Instant::now(), true);
        assert_eq!(commanded(&mut media, Action::PlayPause), None);
    }

    fn controllable(bus: &str, status: PlaybackStatus) -> Player {
        Player {
            capabilities: crate::services::media::Capabilities::all(),
            ..Player::test_player(bus, status)
        }
    }

    #[test]
    fn transport_actions_go_to_the_active_player() {
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, true);
        media.update(vec![controllable("a", PlaybackStatus::Playing)]);
        assert_eq!(
            commanded(&mut media, Action::PlayPause),
            Some("a PlayPause".to_string())
        );
        assert_eq!(
            commanded(&mut media, Action::Next),
            Some("a Next".to_string())
        );
        assert_eq!(
            commanded(&mut media, Action::Previous),
            Some("a Previous".to_string())
        );
    }

    #[test]
    fn an_unsupported_control_is_silently_ignored() {
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, true);
        media.update(vec![player("a", PlaybackStatus::Playing, None, t0)]);
        assert_eq!(commanded(&mut media, Action::PlayPause), None);
        assert_eq!(commanded(&mut media, Action::Next), None);
        assert_eq!(commanded(&mut media, Action::Previous), None);
    }

    #[test]
    fn loop_and_shuffle_move_to_their_next_state() {
        use crate::services::media::LoopStatus;
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, true);
        let mut p = controllable("a", PlaybackStatus::Playing);
        p.loop_status = Some(LoopStatus::Playlist);
        p.shuffle = Some(true);
        media.update(vec![p]);
        assert_eq!(
            commanded(&mut media, Action::CycleLoop),
            Some("a SetLoopStatus(None)".to_string())
        );
        assert_eq!(
            commanded(&mut media, Action::ToggleShuffle),
            Some("a SetShuffle(false)".to_string())
        );
    }

    #[test]
    fn absent_loop_and_shuffle_take_no_command() {
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, true);
        media.update(vec![controllable("a", PlaybackStatus::Playing)]);
        assert_eq!(commanded(&mut media, Action::CycleLoop), None);
        assert_eq!(commanded(&mut media, Action::ToggleShuffle), None);
    }

    #[test]
    fn seeking_addresses_the_current_track() {
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, true);
        let mut p = controllable("a", PlaybackStatus::Playing);
        media.update(vec![p.clone()]);
        assert_eq!(
            commanded(&mut media, Action::Seek(Duration::from_secs(3))),
            None
        );

        p.track.id = zbus::zvariant::OwnedObjectPath::try_from("/t/1").ok();
        p.track.length = Some(Duration::from_secs(100));
        media.update(vec![p]);
        let (bus, command) = media.apply(Action::Seek(Duration::from_secs(3))).unzip();
        assert_eq!(bus.as_deref(), Some("a"));
        assert!(matches!(
            command,
            Some(Command::SetPosition { track, position })
                if track.as_str() == "/t/1" && position == Duration::from_secs(3)
        ));
    }

    #[test]
    fn seeking_moves_the_active_position_at_once() {
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, true);
        let mut p = controllable("a", PlaybackStatus::Paused);
        p.track.id = zbus::zvariant::OwnedObjectPath::try_from("/t/1").ok();
        p.track.length = Some(Duration::from_secs(100));
        media.update(vec![p]);
        let t1 = t0 + Duration::from_secs(1);
        media.tick(t1);
        media.apply(Action::Seek(Duration::from_secs(60)));
        assert_eq!(
            media.active().map(|p| p.position_at(t1)),
            Some(Duration::from_secs(60))
        );
    }
}
