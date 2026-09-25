//! Everything the bar keeps about media playback between messages: the
//! players the service reported, which one is shown, their covers and the
//! slider's wave.

use std::time::{Duration, Instant};

use iced::widget::image;

use crate::bar::media_panel::{wave_phase, WaveEase};
use crate::services::media::{Command, PlaybackStatus, Player, Selection};
use crate::services::media_art::{Art, ArtCache};

/// What the bar shows for media.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Trigger<'a> {
    Hidden,
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

#[derive(Debug)]
pub struct MediaState {
    players: Vec<Player>,
    selection: Selection,
    art: ArtCache,
    wave: WaveEase,
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
            wave: WaveEase::default(),
            epoch: now,
            now,
            show_when_idle,
        }
    }

    /// Adopt a snapshot from the service. Returns a cover URL to fetch when
    /// the shown track has one that is not cached yet.
    pub fn update(&mut self, players: Vec<Player>, now: Instant) -> Option<String> {
        self.now = now;
        self.selection.observe(&players);
        self.players = players;
        self.follow_active()
    }

    /// Show the next player. Returns a cover URL to fetch, as [`Self::update`].
    pub fn cycle_player(&mut self, now: Instant) -> Option<String> {
        self.now = now;
        self.selection.cycle(&self.players);
        self.follow_active()
    }

    /// Point the wave at the active player and ask for its cover.
    fn follow_active(&mut self) -> Option<String> {
        let active = self.selection.active(&self.players);
        let playing = active.is_some_and(is_playing);
        let art_url = active.and_then(|p| p.track.art_url.clone());
        self.wave.target(playing, self.now);
        art_url.filter(|url| self.art.request(url))
    }

    pub fn art_loaded(&mut self, url: String, art: Art) {
        self.art.insert(url, art);
    }

    /// The command `action` sends, and to which player. `None` when there is
    /// no player or it lacks what the action needs.
    #[must_use]
    pub fn command(&self, action: Action) -> Option<(String, Command)> {
        let player = self.active()?;
        let command = match action {
            Action::PlayPause => Command::PlayPause,
            Action::Next => Command::Next,
            Action::Previous => Command::Previous,
            Action::CycleLoop => Command::SetLoopStatus(player.loop_status?.next()),
            Action::ToggleShuffle => Command::SetShuffle(!player.shuffle?),
            Action::Seek(position) => Command::SetPosition {
                track: player.track.id.clone()?,
                position,
            },
        };
        Some((player.bus_name.clone(), command))
    }

    /// Move the slider to `position` ahead of the player confirming it.
    pub fn seek(&mut self, position: Duration, now: Instant) {
        self.now = now;
        let Some(bus) = self.active().map(|p| p.bus_name.clone()) else {
            return;
        };
        if let Some(player) = self.players.iter_mut().find(|p| p.bus_name == bus) {
            player.position.position = position;
            player.position.sampled_at = now;
        }
    }

    pub const fn tick(&mut self, now: Instant) {
        self.now = now;
    }

    /// The bar entry: the active player while one plays, and otherwise only
    /// when `show_when_idle` asks for it.
    #[must_use]
    pub fn trigger(&self) -> Trigger<'_> {
        let playing = self.players.iter().any(is_playing);
        match self.active() {
            Some(player) if playing || self.show_when_idle => Trigger::Player(player),
            None if self.show_when_idle => Trigger::Idle,
            _ => Trigger::Hidden,
        }
    }

    #[must_use]
    pub fn active(&self) -> Option<&Player> {
        self.selection.active(&self.players)
    }

    #[must_use]
    pub const fn players(&self) -> &[Player] {
        self.players.as_slice()
    }

    /// The active track's cover, once it has loaded.
    #[must_use]
    pub fn art(&self) -> Option<&image::Handle> {
        let url = self.active()?.track.art_url.as_deref()?;
        match self.art.get(url)? {
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
        self.wave.level(self.now)
    }

    #[must_use]
    pub fn wave_phase(&self) -> f32 {
        wave_phase(self.now.saturating_duration_since(self.epoch))
    }

    /// Whether the slider needs frames: while playing, and while the wave is
    /// still flattening after a pause.
    #[must_use]
    pub fn animating(&self) -> bool {
        self.active().is_some_and(is_playing) || self.wave.is_moving(self.now)
    }
}

fn is_playing(player: &Player) -> bool {
    player.status == PlaybackStatus::Playing
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::media::{Capabilities, LoopStatus, PositionSample, Track};

    fn player(bus: &str, status: PlaybackStatus, art: Option<&str>, at: Instant) -> Player {
        Player {
            bus_name: bus.to_string(),
            identity: bus.to_string(),
            status,
            track: Track {
                art_url: art.map(ToString::to_string),
                length: Some(Duration::from_secs(100)),
                ..Track::default()
            },
            position: PositionSample {
                position: Duration::from_secs(10),
                sampled_at: at,
                rate: 1.0,
            },
            capabilities: Capabilities::default(),
            loop_status: None,
            shuffle: None,
        }
    }

    fn loaded() -> Art {
        Art::Loaded(image::Handle::from_rgba(1, 1, vec![0; 4]))
    }

    fn bus(trigger: Trigger<'_>) -> Option<&str> {
        match trigger {
            Trigger::Player(p) => Some(p.bus_name.as_str()),
            Trigger::Hidden | Trigger::Idle => None,
        }
    }

    #[test]
    fn no_player_shows_the_idle_icon_only_when_asked() {
        let t0 = Instant::now();
        assert_eq!(MediaState::new(t0, true).trigger(), Trigger::Idle);
        assert_eq!(MediaState::new(t0, false).trigger(), Trigger::Hidden);
    }

    #[test]
    fn a_paused_player_shows_only_when_asked() {
        let t0 = Instant::now();
        let paused = vec![player("a", PlaybackStatus::Paused, None, t0)];
        let mut shown = MediaState::new(t0, true);
        shown.update(paused.clone(), t0);
        assert_eq!(bus(shown.trigger()), Some("a"));
        let mut hidden = MediaState::new(t0, false);
        hidden.update(paused, t0);
        assert_eq!(hidden.trigger(), Trigger::Hidden);
    }

    #[test]
    fn a_playing_player_always_shows() {
        let t0 = Instant::now();
        for show_when_idle in [true, false] {
            let mut media = MediaState::new(t0, show_when_idle);
            media.update(
                vec![
                    player("a", PlaybackStatus::Stopped, None, t0),
                    player("b", PlaybackStatus::Playing, None, t0),
                ],
                t0,
            );
            assert_eq!(bus(media.trigger()), Some("b"));
        }
    }

    #[test]
    fn any_playing_player_keeps_the_entry_up() {
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, false);
        media.update(
            vec![
                player("a", PlaybackStatus::Playing, None, t0),
                player("b", PlaybackStatus::Paused, None, t0),
            ],
            t0,
        );
        media.cycle_player(t0);
        assert_eq!(bus(media.trigger()), Some("b"));
    }

    #[test]
    fn nothing_is_shown_without_players() {
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, true);
        assert_eq!(media.update(Vec::new(), t0), None);
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
            media.update(players.clone(), t0),
            Some("file:///a.png".to_string())
        );
        assert_eq!(media.update(players, t0), None);
    }

    #[test]
    fn a_loaded_cover_is_shown_for_its_track_only() {
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, true);
        media.update(
            vec![player("a", PlaybackStatus::Paused, Some("u1"), t0)],
            t0,
        );
        media.art_loaded("u1".to_string(), loaded());
        assert!(media.art().is_some());

        let next = media.update(
            vec![player("a", PlaybackStatus::Paused, Some("u2"), t0)],
            t0,
        );
        assert_eq!(next, Some("u2".to_string()));
        assert!(media.art().is_none());
    }

    #[test]
    fn a_failed_cover_shows_no_art() {
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, true);
        media.update(
            vec![player("a", PlaybackStatus::Paused, Some("u1"), t0)],
            t0,
        );
        media.art_loaded("u1".to_string(), Art::Failed);
        assert!(media.art().is_none());
    }

    #[test]
    fn cycling_requests_the_new_players_cover() {
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, true);
        media.update(
            vec![
                player("a", PlaybackStatus::Playing, Some("ua"), t0),
                player("b", PlaybackStatus::Paused, Some("ub"), t0),
            ],
            t0,
        );
        assert_eq!(media.cycle_player(t0), Some("ub".to_string()));
        assert_eq!(media.active().map(|p| p.bus_name.as_str()), Some("b"));
    }

    #[test]
    fn playing_animates_and_a_pause_settles_after_the_ease() {
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, true);
        media.update(vec![player("a", PlaybackStatus::Playing, None, t0)], t0);
        assert!(media.animating());

        let t1 = t0 + Duration::from_secs(5);
        media.update(vec![player("a", PlaybackStatus::Paused, None, t1)], t1);
        assert!(media.animating());
        media.tick(t1 + Duration::from_secs(5));
        assert!(!media.animating());
        assert!(media.wave_level().abs() < f32::EPSILON);
    }

    fn commanded(media: &MediaState, action: Action) -> Option<String> {
        media
            .command(action)
            .map(|(bus, command)| format!("{bus} {command:?}"))
    }

    #[test]
    fn no_player_takes_no_command() {
        let media = MediaState::new(Instant::now(), true);
        assert_eq!(commanded(&media, Action::PlayPause), None);
    }

    #[test]
    fn transport_actions_go_to_the_active_player() {
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, true);
        media.update(vec![player("a", PlaybackStatus::Playing, None, t0)], t0);
        assert_eq!(
            commanded(&media, Action::PlayPause),
            Some("a PlayPause".to_string())
        );
        assert_eq!(commanded(&media, Action::Next), Some("a Next".to_string()));
        assert_eq!(
            commanded(&media, Action::Previous),
            Some("a Previous".to_string())
        );
    }

    #[test]
    fn loop_and_shuffle_move_to_their_next_state() {
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, true);
        let mut p = player("a", PlaybackStatus::Playing, None, t0);
        p.loop_status = Some(LoopStatus::Playlist);
        p.shuffle = Some(true);
        media.update(vec![p], t0);
        assert_eq!(
            commanded(&media, Action::CycleLoop),
            Some("a SetLoopStatus(None)".to_string())
        );
        assert_eq!(
            commanded(&media, Action::ToggleShuffle),
            Some("a SetShuffle(false)".to_string())
        );
    }

    #[test]
    fn absent_loop_and_shuffle_take_no_command() {
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, true);
        media.update(vec![player("a", PlaybackStatus::Playing, None, t0)], t0);
        assert_eq!(commanded(&media, Action::CycleLoop), None);
        assert_eq!(commanded(&media, Action::ToggleShuffle), None);
    }

    #[test]
    fn seeking_addresses_the_current_track() {
        let t0 = Instant::now();
        let mut media = MediaState::new(t0, true);
        let mut p = player("a", PlaybackStatus::Playing, None, t0);
        media.update(vec![p.clone()], t0);
        assert_eq!(
            commanded(&media, Action::Seek(Duration::from_secs(3))),
            None
        );

        p.track.id = zbus::zvariant::OwnedObjectPath::try_from("/t/1").ok();
        media.update(vec![p], t0);
        let (bus, command) = media.command(Action::Seek(Duration::from_secs(3))).unzip();
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
        media.update(vec![player("a", PlaybackStatus::Paused, None, t0)], t0);
        let t1 = t0 + Duration::from_secs(1);
        media.seek(Duration::from_secs(60), t1);
        assert_eq!(
            media.active().map(|p| p.position_at(t1)),
            Some(Duration::from_secs(60))
        );
    }
}
