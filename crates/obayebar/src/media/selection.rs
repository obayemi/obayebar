//! Which player the bar and the panel show.

use std::collections::HashMap;

use crate::services::media::{PlaybackStatus, Player};

/// Where the active player sits among several, in cycling order: the
/// `index`th, counted from one, of `total`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rotation {
    pub index: usize,
    pub total: usize,
}

/// The player that most recently started playing wins; before any player has
/// played, the one that appeared last. Cycling from the panel chip pins a
/// player, and the pin holds until some *other* player starts playing or the
/// pinned player itself disappears.
#[derive(Debug, Default)]
pub struct Selection {
    statuses: HashMap<String, PlaybackStatus>,
    /// Bus names in the order they started playing, most recent last. A bus
    /// name that starts again moves to the end rather than duplicating.
    started: Vec<String>,
    last_seen: Option<String>,
    pinned: Option<String>,
}

impl Selection {
    /// Record a fresh snapshot, noticing appearances and starts, and forget
    /// every reference to a player no longer in it.
    pub fn observe(&mut self, players: &[Player]) {
        for player in players {
            let previous = self.statuses.insert(player.bus_name.clone(), player.status);
            if previous.is_none() {
                self.last_seen = Some(player.bus_name.clone());
            }
            let started = player.is_playing() && previous != Some(PlaybackStatus::Playing);
            if started {
                self.started.retain(|bus| bus != &player.bus_name);
                self.started.push(player.bus_name.clone());
                if self.pinned.as_deref() != Some(player.bus_name.as_str()) {
                    self.pinned = None;
                }
            }
        }
        let present = |bus: &str| players.iter().any(|p| p.bus_name == bus);
        self.statuses.retain(|bus, _| present(bus));
        self.started.retain(|bus| present(bus));
        if !self.last_seen.as_deref().is_some_and(present) {
            self.last_seen = None;
        }
        if !self.pinned.as_deref().is_some_and(present) {
            self.pinned = None;
        }
    }

    #[must_use]
    pub fn active<'a>(&self, players: &'a [Player]) -> Option<&'a Player> {
        let by_bus = |bus: &str| players.iter().find(|p| p.bus_name == bus);
        self.pinned
            .as_deref()
            .and_then(by_bus)
            .or_else(|| self.started.iter().rev().find_map(|bus| by_bus(bus)))
            .or_else(|| self.last_seen.as_deref().and_then(by_bus))
            .or_else(|| players.iter().find(|p| p.is_playing()))
            .or_else(|| players.first())
    }

    /// Pin the player after the active one, wrapping around.
    pub fn cycle(&mut self, players: &[Player]) {
        let next = self
            .active_position(players)
            .and_then(|i| players.get(i.checked_add(1)?))
            .or_else(|| players.first());
        self.pinned = next.map(|p| p.bus_name.clone());
    }

    /// The active player's place among the others. `None` unless there is
    /// another player to cycle to.
    #[must_use]
    pub fn rotation(&self, players: &[Player]) -> Option<Rotation> {
        let total = players.len();
        if total < 2 {
            return None;
        }
        let index = self.active_position(players)?.checked_add(1)?;
        Some(Rotation { index, total })
    }

    fn active_position(&self, players: &[Player]) -> Option<usize> {
        let active = self.active(players)?;
        players.iter().position(|p| p.bus_name == active.bus_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(bus: &str, status: PlaybackStatus) -> Player {
        Player::test_player(bus, status)
    }

    fn active_bus(selection: &Selection, players: &[Player]) -> Option<String> {
        selection.active(players).map(|p| p.bus_name.clone())
    }

    #[test]
    fn no_players_means_no_active_player() {
        let mut selection = Selection::default();
        selection.observe(&[]);
        assert_eq!(active_bus(&selection, &[]), None);
    }

    #[test]
    fn falls_back_to_the_last_player_seen() {
        let mut selection = Selection::default();
        let first = [player("a", PlaybackStatus::Paused)];
        selection.observe(&first);
        let both = [
            player("a", PlaybackStatus::Paused),
            player("b", PlaybackStatus::Stopped),
        ];
        selection.observe(&both);
        assert_eq!(active_bus(&selection, &both), Some("b".to_string()));
    }

    #[test]
    fn the_most_recent_player_to_start_wins() {
        let mut selection = Selection::default();
        let a_plays = [
            player("a", PlaybackStatus::Playing),
            player("b", PlaybackStatus::Paused),
        ];
        selection.observe(&a_plays);
        assert_eq!(active_bus(&selection, &a_plays), Some("a".to_string()));

        let both_play = [
            player("a", PlaybackStatus::Playing),
            player("b", PlaybackStatus::Playing),
        ];
        selection.observe(&both_play);
        assert_eq!(active_bus(&selection, &both_play), Some("b".to_string()));
    }

    #[test]
    fn pausing_keeps_the_player_active() {
        let mut selection = Selection::default();
        selection.observe(&[
            player("a", PlaybackStatus::Playing),
            player("b", PlaybackStatus::Paused),
        ]);
        let paused = [
            player("a", PlaybackStatus::Paused),
            player("b", PlaybackStatus::Paused),
        ];
        selection.observe(&paused);
        assert_eq!(active_bus(&selection, &paused), Some("a".to_string()));
    }

    #[test]
    fn a_vanished_active_player_hands_over() {
        let mut selection = Selection::default();
        selection.observe(&[
            player("a", PlaybackStatus::Paused),
            player("b", PlaybackStatus::Playing),
        ]);
        let left = [player("a", PlaybackStatus::Paused)];
        selection.observe(&left);
        assert_eq!(active_bus(&selection, &left), Some("a".to_string()));
    }

    #[test]
    fn cycling_pins_the_next_player_and_wraps() {
        let mut selection = Selection::default();
        let players = [
            player("a", PlaybackStatus::Playing),
            player("b", PlaybackStatus::Paused),
            player("c", PlaybackStatus::Paused),
        ];
        selection.observe(&players);
        selection.cycle(&players);
        assert_eq!(active_bus(&selection, &players), Some("b".to_string()));
        selection.cycle(&players);
        assert_eq!(active_bus(&selection, &players), Some("c".to_string()));
        selection.cycle(&players);
        assert_eq!(active_bus(&selection, &players), Some("a".to_string()));
    }

    #[test]
    fn a_lone_player_has_no_rotation() {
        let mut selection = Selection::default();
        assert_eq!(selection.rotation(&[]), None);
        let alone = [player("a", PlaybackStatus::Playing)];
        selection.observe(&alone);
        assert_eq!(selection.rotation(&alone), None);
    }

    #[test]
    fn the_rotation_follows_the_active_player() {
        let mut selection = Selection::default();
        let players = [
            player("a", PlaybackStatus::Playing),
            player("b", PlaybackStatus::Paused),
            player("c", PlaybackStatus::Paused),
        ];
        selection.observe(&players);
        assert_eq!(
            selection.rotation(&players),
            Some(Rotation { index: 1, total: 3 })
        );
        selection.cycle(&players);
        assert_eq!(
            selection.rotation(&players),
            Some(Rotation { index: 2, total: 3 })
        );
    }

    #[test]
    fn a_pin_survives_updates_until_another_player_starts() {
        let mut selection = Selection::default();
        let players = [
            player("a", PlaybackStatus::Playing),
            player("b", PlaybackStatus::Paused),
            player("c", PlaybackStatus::Paused),
        ];
        selection.observe(&players);
        selection.cycle(&players);
        selection.observe(&players);
        assert_eq!(active_bus(&selection, &players), Some("b".to_string()));

        let c_starts = [
            player("a", PlaybackStatus::Playing),
            player("b", PlaybackStatus::Paused),
            player("c", PlaybackStatus::Playing),
        ];
        selection.observe(&c_starts);
        assert_eq!(active_bus(&selection, &c_starts), Some("c".to_string()));
    }

    #[test]
    fn the_pinned_player_starting_keeps_its_pin() {
        let mut selection = Selection::default();
        let players = [
            player("a", PlaybackStatus::Playing),
            player("b", PlaybackStatus::Paused),
        ];
        selection.observe(&players);
        selection.cycle(&players);
        let b_starts = [
            player("a", PlaybackStatus::Playing),
            player("b", PlaybackStatus::Playing),
        ];
        selection.observe(&b_starts);
        assert_eq!(active_bus(&selection, &b_starts), Some("b".to_string()));
    }

    #[test]
    fn a_relaunched_player_does_not_steal_the_active_one_it_never_started() {
        let mut selection = Selection::default();
        selection.observe(&[
            player("spotify", PlaybackStatus::Playing),
            player("firefox", PlaybackStatus::Paused),
        ]);
        let firefox_starts = [
            player("spotify", PlaybackStatus::Playing),
            player("firefox", PlaybackStatus::Playing),
        ];
        selection.observe(&firefox_starts);
        let firefox_quits = [player("spotify", PlaybackStatus::Playing)];
        selection.observe(&firefox_quits);
        assert_eq!(
            active_bus(&selection, &firefox_quits),
            Some("spotify".to_string())
        );

        let firefox_relaunches_paused = [
            player("spotify", PlaybackStatus::Playing),
            player("firefox", PlaybackStatus::Paused),
        ];
        selection.observe(&firefox_relaunches_paused);
        assert_eq!(
            active_bus(&selection, &firefox_relaunches_paused),
            Some("spotify".to_string())
        );
    }

    #[test]
    fn a_disappearing_pin_is_dropped_rather_than_reclaimed_on_relaunch() {
        let mut selection = Selection::default();
        let players = [
            player("firefox", PlaybackStatus::Playing),
            player("spotify", PlaybackStatus::Paused),
        ];
        selection.observe(&players);
        selection.cycle(&players);
        assert_eq!(
            active_bus(&selection, &players),
            Some("spotify".to_string())
        );

        let spotify_quits = [player("firefox", PlaybackStatus::Playing)];
        selection.observe(&spotify_quits);
        assert_eq!(
            active_bus(&selection, &spotify_quits),
            Some("firefox".to_string())
        );

        let spotify_relaunches_paused = [
            player("firefox", PlaybackStatus::Playing),
            player("spotify", PlaybackStatus::Paused),
        ];
        selection.observe(&spotify_relaunches_paused);
        assert_eq!(
            active_bus(&selection, &spotify_relaunches_paused),
            Some("firefox".to_string())
        );
    }
}
