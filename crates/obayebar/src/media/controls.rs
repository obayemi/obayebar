//! What the panel offers for the active player, and what pressing it sends.

use std::time::Duration;

use zbus::zvariant::OwnedObjectPath;

use crate::services::media::{Capabilities, LoopStatus, Player};

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
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Controls {
    pub previous: bool,
    pub next: bool,
    pub play_pause: Option<PlayPause>,
    pub loop_status: Option<LoopStatus>,
    pub shuffle: Option<bool>,
    pub seek: Option<SeekTarget>,
}

impl Controls {
    /// A player reporting `CanControl` false accepts no command at all, so
    /// it shows none.
    #[must_use]
    pub fn for_player(player: &Player) -> Self {
        let caps = player.capabilities;
        if !caps.contains(Capabilities::CONTROL) {
            return Self::default();
        }
        Self {
            previous: caps.contains(Capabilities::GO_PREVIOUS),
            next: caps.contains(Capabilities::GO_NEXT),
            play_pause: (caps.contains(Capabilities::PLAY) || caps.contains(Capabilities::PAUSE))
                .then_some(if player.is_playing() {
                    PlayPause::Pause
                } else {
                    PlayPause::Play
                }),
            loop_status: player.loop_status,
            shuffle: player.shuffle,
            seek: caps
                .contains(Capabilities::SEEK)
                .then(|| player.track.id.clone().zip(player.track.length))
                .flatten()
                .map(|(track, length)| SeekTarget { track, length }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::media::PlaybackStatus;

    fn track_id() -> OwnedObjectPath {
        OwnedObjectPath::try_from("/track/1").unwrap_or_else(|e| unreachable!("{e}"))
    }

    fn player(capabilities: Capabilities) -> Player {
        Player {
            capabilities,
            loop_status: Some(LoopStatus::Track),
            shuffle: Some(false),
            track: crate::services::media::Track {
                id: Some(track_id()),
                length: Some(Duration::from_secs(200)),
                ..crate::services::media::Track::default()
            },
            ..Player::test_player("org.mpris.MediaPlayer2.test", PlaybackStatus::Playing)
        }
    }

    fn without(missing: &[Capabilities]) -> Capabilities {
        missing.iter().fold(Capabilities::all(), |caps, missing| {
            caps.difference(*missing)
        })
    }

    #[test]
    fn a_fully_capable_player_shows_every_control() {
        let controls = Controls::for_player(&player(Capabilities::all()));
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
        let controls = Controls::for_player(&player(without(&[Capabilities::GO_PREVIOUS])));
        assert!(!controls.previous);
        assert!(controls.next);
        let controls = Controls::for_player(&player(without(&[Capabilities::GO_NEXT])));
        assert!(controls.previous);
        assert!(!controls.next);
    }

    #[test]
    fn play_pause_needs_either_capability() {
        let neither = without(&[Capabilities::PLAY, Capabilities::PAUSE]);
        assert_eq!(Controls::for_player(&player(neither)).play_pause, None);

        let pause_only = Capabilities::CONTROL | Capabilities::PAUSE;
        assert_eq!(
            Controls::for_player(&player(pause_only)).play_pause,
            Some(PlayPause::Pause)
        );

        let mut paused = player(Capabilities::CONTROL | Capabilities::PLAY);
        paused.status = PlaybackStatus::Paused;
        assert_eq!(
            Controls::for_player(&paused).play_pause,
            Some(PlayPause::Play)
        );
    }

    #[test]
    fn absent_loop_and_shuffle_are_hidden() {
        let mut p = player(Capabilities::all());
        p.loop_status = None;
        p.shuffle = None;
        let controls = Controls::for_player(&p);
        assert_eq!(controls.loop_status, None);
        assert_eq!(controls.shuffle, None);
    }

    #[test]
    fn a_player_that_cannot_be_controlled_shows_nothing() {
        let controls = Controls::for_player(&player(without(&[Capabilities::CONTROL])));
        assert_eq!(controls, Controls::default());
    }

    #[test]
    fn seeking_needs_the_capability_a_track_id_and_a_length() {
        assert_eq!(
            Controls::for_player(&player(without(&[Capabilities::SEEK]))).seek,
            None
        );
        let mut no_id = player(Capabilities::all());
        no_id.track.id = None;
        assert_eq!(Controls::for_player(&no_id).seek, None);
        let mut no_length = player(Capabilities::all());
        no_length.track.length = None;
        assert_eq!(Controls::for_player(&no_length).seek, None);
    }
}
