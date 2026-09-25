//! MPRIS client: every `org.mpris.MediaPlayer2.*` name on the session bus,
//! read into [`Player`] snapshots, plus the commands the media panel sends.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use bitflags::bitflags;
use futures_util::stream::StreamExt;
use futures_util::Stream;
use zbus::names::{InterfaceName, OwnedUniqueName};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use crate::services::dbus_util::{self, PanelSignal};

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const ROOT_IFACE: &str = "org.mpris.MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";
/// The track id MPRIS reserves for "no track".
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";
/// `playerctld` re-exports whichever player last had focus under its own
/// MPRIS name, which is not a player to list alongside the one it mirrors.
const PLAYERCTLD: &str = "org.mpris.MediaPlayer2.playerctld";

/// How often `Position` is re-read while the panel shows a playing player, to
/// correct the drift of the extrapolation.
const RESAMPLE_INTERVAL: Duration = Duration::from_secs(4);
/// Longest a single property read may take before it counts as failed, so one
/// unresponsive player cannot stall every other player's signals.
const READ_TIMEOUT: Duration = Duration::from_secs(2);

static PANEL: PanelSignal = PanelSignal::new();

/// Toggle from the UI when the media panel opens/closes. Position is only
/// resampled while it is open, since nothing else shows it.
pub fn set_panel_open(open: bool) {
    PANEL.set(open);
}

/// Whether `name` is a real MPRIS player rather than an aggregator such as
/// `playerctld`, which re-exports another player's name under its own.
fn is_player_name(name: &str) -> bool {
    name.starts_with(MPRIS_PREFIX) && name != PLAYERCTLD
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
}

impl PlaybackStatus {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "Playing" => Some(Self::Playing),
            "Paused" => Some(Self::Paused),
            "Stopped" => Some(Self::Stopped),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopStatus {
    None,
    Track,
    Playlist,
}

impl LoopStatus {
    const ALL: [Self; 3] = [Self::None, Self::Track, Self::Playlist];

    fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.as_str() == raw)
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Track => "Track",
            Self::Playlist => "Playlist",
        }
    }

    /// The state the panel's loop button moves to: None → Track → Playlist →
    /// None.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::None => Self::Track,
            Self::Track => Self::Playlist,
            Self::Playlist => Self::None,
        }
    }
}

bitflags! {
    /// The `Can*` properties of a player, as a set.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct Capabilities: u8 {
        const GO_NEXT = 1 << 0;
        const GO_PREVIOUS = 1 << 1;
        const PLAY = 1 << 2;
        const PAUSE = 1 << 3;
        const SEEK = 1 << 4;
        const CONTROL = 1 << 5;
    }
}

/// The `Can*` property names, in the order their bit is declared.
const CAPABILITY_PROPERTIES: [(Capabilities, &str); 6] = [
    (Capabilities::GO_NEXT, "CanGoNext"),
    (Capabilities::GO_PREVIOUS, "CanGoPrevious"),
    (Capabilities::PLAY, "CanPlay"),
    (Capabilities::PAUSE, "CanPause"),
    (Capabilities::SEEK, "CanSeek"),
    (Capabilities::CONTROL, "CanControl"),
];

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Track {
    pub id: Option<OwnedObjectPath>,
    pub title: Option<String>,
    pub artists: Vec<String>,
    pub length: Option<Duration>,
    pub art_url: Option<String>,
}

impl Track {
    /// The artists, joined for display; `None` when there are none.
    #[must_use]
    pub fn artists_line(&self) -> Option<String> {
        (!self.artists.is_empty()).then(|| self.artists.join(", "))
    }
}

/// A `Position` read and the moment it was read. MPRIS never signals position
/// changes, so the current position is extrapolated from this.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionSample {
    pub position: Duration,
    pub sampled_at: Instant,
    pub rate: f64,
}

/// Where playback is at `now`: the sample advanced by the elapsed time times
/// the rate while playing, frozen otherwise, never past the track's end.
#[must_use]
pub fn extrapolate(
    sample: PositionSample,
    status: PlaybackStatus,
    length: Option<Duration>,
    now: Instant,
) -> Duration {
    if status != PlaybackStatus::Playing {
        return sample.position;
    }
    let elapsed = now.saturating_duration_since(sample.sampled_at);
    let advanced =
        Duration::try_from_secs_f64(elapsed.as_secs_f64() * sample.rate).unwrap_or(Duration::ZERO);
    let position = sample.position.saturating_add(advanced);
    length.map_or(position, |length| position.min(length))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Player {
    pub bus_name: String,
    pub identity: String,
    pub status: PlaybackStatus,
    pub track: Track,
    pub position: PositionSample,
    pub capabilities: Capabilities,
    /// `None` when the player does not implement the optional property.
    pub loop_status: Option<LoopStatus>,
    /// `None` when the player does not implement the optional property.
    pub shuffle: Option<bool>,
}

impl Player {
    #[must_use]
    pub fn position_at(&self, now: Instant) -> Duration {
        extrapolate(self.position, self.status, self.track.length, now)
    }

    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.status == PlaybackStatus::Playing
    }

    /// The track's title, or the player's identity while it has none.
    #[must_use]
    pub fn title(&self) -> &str {
        self.track.title.as_deref().unwrap_or(&self.identity)
    }

    /// A player fixture for tests: an untitled, unpositioned track with no
    /// capabilities, built with struct update syntax from the field or two a
    /// test actually cares about.
    #[cfg(test)]
    pub(crate) fn test_player(bus: &str, status: PlaybackStatus) -> Self {
        Self {
            bus_name: bus.to_string(),
            identity: bus.to_string(),
            status,
            track: Track::default(),
            position: PositionSample {
                position: Duration::ZERO,
                sampled_at: Instant::now(),
                rate: 1.0,
            },
            capabilities: Capabilities::default(),
            loop_status: None,
            shuffle: None,
        }
    }
}

/// Build a [`Player`] from the `GetAll` reply of `org.mpris.MediaPlayer2.Player`.
fn parse_player(
    bus_name: &str,
    identity: &str,
    props: &HashMap<String, OwnedValue>,
    sampled_at: Instant,
) -> Player {
    let get = |key: &str| props.get(key);
    let capabilities = CAPABILITY_PROPERTIES
        .into_iter()
        .filter(|(_, key)| get(key).and_then(as_bool) == Some(true))
        .fold(Capabilities::empty(), |caps, (capability, _)| {
            caps | capability
        });
    Player {
        bus_name: bus_name.to_string(),
        identity: identity.to_string(),
        status: get("PlaybackStatus")
            .and_then(as_string)
            .and_then(|raw| PlaybackStatus::parse(&raw))
            .unwrap_or(PlaybackStatus::Stopped),
        track: get("Metadata").map(parse_track).unwrap_or_default(),
        position: PositionSample {
            position: get("Position").and_then(as_micros).unwrap_or_default(),
            sampled_at,
            rate: get("Rate")
                .and_then(|v| v.downcast_ref::<f64>().ok())
                .unwrap_or(1.0),
        },
        capabilities,
        loop_status: get("LoopStatus")
            .and_then(as_string)
            .and_then(|raw| LoopStatus::parse(&raw)),
        shuffle: get("Shuffle").and_then(as_bool),
    }
}

/// Parse the `a{sv}` `Metadata` property.
fn parse_track(metadata: &OwnedValue) -> Track {
    let Ok(map) = metadata
        .try_clone()
        .and_then(HashMap::<String, OwnedValue>::try_from)
    else {
        return Track::default();
    };
    let get = |key: &str| map.get(key);
    Track {
        id: get("mpris:trackid")
            .and_then(as_object_path)
            .filter(|id| id.as_str() != NO_TRACK),
        title: get("xesam:title")
            .and_then(as_string)
            .filter(|title| !title.is_empty()),
        artists: get("xesam:artist").map(as_strings).unwrap_or_default(),
        length: get("mpris:length")
            .and_then(as_micros)
            .filter(|length| !length.is_zero()),
        art_url: get("mpris:artUrl")
            .and_then(as_string)
            .filter(|url| !url.is_empty()),
    }
}

fn as_bool(value: &OwnedValue) -> Option<bool> {
    value.downcast_ref::<bool>().ok()
}

fn as_string(value: &OwnedValue) -> Option<String> {
    value.downcast_ref::<String>().ok()
}

/// `xesam:artist` is specified as `as`, but some players send a bare string.
fn as_strings(value: &OwnedValue) -> Vec<String> {
    as_string(value).map_or_else(
        || {
            value
                .try_clone()
                .and_then(Vec::<String>::try_from)
                .unwrap_or_default()
        },
        |single| vec![single],
    )
}

/// A microsecond count, sent as `x` per the spec or as `t` by some players.
fn as_micros(value: &OwnedValue) -> Option<Duration> {
    let micros = value
        .downcast_ref::<i64>()
        .ok()
        .and_then(|signed| u64::try_from(signed).ok())
        .or_else(|| value.downcast_ref::<u64>().ok())?;
    Some(Duration::from_micros(micros))
}

/// `mpris:trackid` is specified as `o`, but browsers send it as a string.
fn as_object_path(value: &OwnedValue) -> Option<OwnedObjectPath> {
    value
        .downcast_ref::<zbus::zvariant::ObjectPath<'_>>()
        .ok()
        .map(OwnedObjectPath::from)
        .or_else(|| as_string(value).and_then(|raw| OwnedObjectPath::try_from(raw).ok()))
}

/// The name to show for a player whose `Identity` could not be read.
fn fallback_identity(bus_name: &str) -> &str {
    bus_name
        .strip_prefix(MPRIS_PREFIX)
        .and_then(|rest| rest.split('.').next())
        .unwrap_or(bus_name)
}

#[derive(Debug, Clone)]
pub enum Command {
    PlayPause,
    Next,
    Previous,
    SetPosition {
        track: OwnedObjectPath,
        position: Duration,
    },
    SetLoopStatus(LoopStatus),
    SetShuffle(bool),
}

/// Send `command` to the player owning `bus_name`, logging a failure.
pub fn send(bus_name: String, command: Command) {
    tokio::spawn(async move {
        let action = format!("{command:?} on {bus_name}");
        match run_command(&bus_name, command).await {
            Ok(()) => log::debug!("media: {action} succeeded"),
            Err(e) => log::warn!("media: {action} failed: {e}"),
        }
    });
}

async fn run_command(
    bus_name: &str,
    command: Command,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let conn = zbus::Connection::session().await?;
    let proxy = dbus_util::try_proxy(&conn, bus_name, MPRIS_PATH, PLAYER_IFACE).await?;
    match command {
        Command::PlayPause => proxy.call::<_, _, ()>("PlayPause", &()).await?,
        Command::Next => proxy.call::<_, _, ()>("Next", &()).await?,
        Command::Previous => proxy.call::<_, _, ()>("Previous", &()).await?,
        Command::SetPosition { track, position } => {
            let micros = i64::try_from(position.as_micros())?;
            proxy
                .call::<_, _, ()>("SetPosition", &(track, micros))
                .await?;
        }
        Command::SetLoopStatus(status) => proxy.set_property("LoopStatus", status.as_str()).await?,
        Command::SetShuffle(shuffle) => proxy.set_property("Shuffle", shuffle).await?,
    }
    Ok(())
}

/// A player on the bus, with the unique name that owns it — signals carry the
/// unique sender, never the well-known name — and the last successful read,
/// kept until a read under the *same* owner replaces it. `None` only while a
/// fresh owner has not answered yet.
#[derive(Debug, Clone, PartialEq)]
struct Tracked {
    owner: OwnedUniqueName,
    player: Option<Player>,
}

/// What `bus_name`'s entry becomes after one read attempt: the fresh player
/// on success; on failure, the previous read if `owner` is unchanged, and
/// unread otherwise, since a new owner's silence says nothing about what the
/// old one was playing.
fn resolve_read(
    previous: Option<&Tracked>,
    owner: &OwnedUniqueName,
    read: Option<Player>,
) -> Tracked {
    let kept = previous
        .filter(|tracked| &tracked.owner == owner)
        .and_then(|tracked| tracked.player.clone());
    Tracked {
        owner: owner.clone(),
        player: read.or(kept),
    }
}

async fn read_player(
    conn: &zbus::Connection,
    bus_name: &str,
    identity: Option<String>,
) -> Option<Player> {
    tokio::time::timeout(READ_TIMEOUT, read_player_untimed(conn, bus_name, identity))
        .await
        .ok()
        .flatten()
}

async fn read_player_untimed(
    conn: &zbus::Connection,
    bus_name: &str,
    identity: Option<String>,
) -> Option<Player> {
    let props = dbus_util::properties_proxy(conn, bus_name, MPRIS_PATH).await?;
    let player_props = props
        .get_all(InterfaceName::from_static_str_unchecked(PLAYER_IFACE))
        .await
        .map_err(|e| log::warn!("media: reading {bus_name} failed: {e}"))
        .ok()?;
    let identity = match identity {
        Some(identity) => identity,
        None => props
            .get(
                InterfaceName::from_static_str_unchecked(ROOT_IFACE),
                "Identity",
            )
            .await
            .ok()
            .as_ref()
            .and_then(as_string)
            .unwrap_or_else(|| fallback_identity(bus_name).to_string()),
    };
    Some(parse_player(
        bus_name,
        &identity,
        &player_props,
        Instant::now(),
    ))
}

/// Read `bus_name` into `players`, keeping its previous read rather than
/// dropping the entry when the fresh read fails.
async fn refresh_player(
    conn: &zbus::Connection,
    players: &mut BTreeMap<String, Tracked>,
    bus_name: &str,
    owner: OwnedUniqueName,
) {
    let previous = players.get(bus_name);
    let identity = previous
        .filter(|tracked| tracked.owner == owner)
        .and_then(|tracked| tracked.player.as_ref())
        .map(|player| player.identity.clone());
    let read = read_player(conn, bus_name, identity).await;
    players.insert(bus_name.to_string(), resolve_read(previous, &owner, read));
}

async fn refresh_where(
    conn: &zbus::Connection,
    players: &mut BTreeMap<String, Tracked>,
    keep: impl Fn(&Tracked) -> bool,
) {
    let stale: Vec<(String, OwnedUniqueName)> = players
        .iter()
        .filter(|(_, tracked)| keep(tracked))
        .map(|(bus, tracked)| (bus.clone(), tracked.owner.clone()))
        .collect();
    for (bus, owner) in stale {
        refresh_player(conn, players, &bus, owner).await;
    }
}

async fn scan(
    conn: &zbus::Connection,
    dbus: &zbus::fdo::DBusProxy<'_>,
) -> BTreeMap<String, Tracked> {
    let mut players = BTreeMap::new();
    let names = dbus
        .list_names()
        .await
        .map_err(|e| log::warn!("media: ListNames failed: {e}"))
        .unwrap_or_default();
    for name in names.iter().filter(|n| is_player_name(n)) {
        let Ok(owner) = dbus.get_name_owner(name.inner().clone()).await else {
            continue;
        };
        refresh_player(conn, &mut players, name, owner).await;
    }
    players
}

fn snapshot(players: &BTreeMap<String, Tracked>) -> Vec<Player> {
    players.values().filter_map(|t| t.player.clone()).collect()
}

pub fn stream() -> impl Stream<Item = Vec<Player>> {
    dbus_util::spawn_stream(
        "media",
        dbus_util::Bus::Session,
        Duration::from_secs(5),
        |conn, tx| async move { run_media_loop(&conn, &tx).await },
    )
}

async fn run_media_loop(
    conn: &zbus::Connection,
    tx: &tokio::sync::mpsc::UnboundedSender<Vec<Player>>,
) -> Result<(), ()> {
    let dbus = zbus::fdo::DBusProxy::new(conn).await.map_err(|_| ())?;
    let mut owner_changes = dbus.receive_name_owner_changed().await.map_err(|_| ())?;
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .path(MPRIS_PATH)
        .map_err(|_| ())?
        .build();
    let mut player_signals = zbus::MessageStream::for_match_rule(rule, conn, None)
        .await
        .map_err(|_| ())?;

    let mut players = scan(conn, &dbus).await;
    let mut last = snapshot(&players);
    tx.send(last.clone()).map_err(|_| ())?;

    loop {
        let resample = PANEL.is_open()
            && players
                .values()
                .filter_map(|t| t.player.as_ref())
                .any(Player::is_playing);
        tokio::select! {
            Some(change) = owner_changes.next() => {
                let Ok(args) = change.args() else { continue };
                let name = args.name().to_string();
                if !is_player_name(&name) {
                    continue;
                }
                match args.new_owner().as_ref() {
                    Some(owner) => refresh_player(conn, &mut players, &name, OwnedUniqueName::from(owner.clone())).await,
                    None => {
                        players.remove(&name);
                    }
                }
            }
            Some(Ok(signal)) = player_signals.next() => {
                let header = signal.header();
                let Some(sender) = header.sender() else { continue };
                refresh_where(conn, &mut players, |t| &t.owner == sender).await;
            }
            () = PANEL.changed() => refresh_where(conn, &mut players, |_| true).await,
            () = tokio::time::sleep(RESAMPLE_INTERVAL), if resample => {
                refresh_where(conn, &mut players, |t| t.player.as_ref().is_some_and(Player::is_playing)).await;
            }
        }
        dbus_util::send_if_changed(tx, &mut last, snapshot(&players))?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::{Dict, Value};

    fn owned(value: Value<'_>) -> OwnedValue {
        OwnedValue::try_from(value).unwrap_or_else(|e| unreachable!("{e}"))
    }

    fn metadata(entries: Vec<(&str, Value<'static>)>) -> OwnedValue {
        let map: HashMap<String, Value<'static>> = entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        owned(Value::from(Dict::from(map)))
    }

    fn props(entries: Vec<(&str, OwnedValue)>) -> HashMap<String, OwnedValue> {
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    fn owned_unique(name: &str) -> OwnedUniqueName {
        OwnedUniqueName::try_from(name).unwrap_or_else(|e| unreachable!("{e}"))
    }

    fn full_props() -> HashMap<String, OwnedValue> {
        props(vec![
            ("PlaybackStatus", owned(Value::from("Playing"))),
            (
                "Metadata",
                metadata(vec![
                    ("xesam:title", Value::from("Song")),
                    ("xesam:artist", Value::from(vec!["A", "B"])),
                    ("mpris:length", Value::from(180_000_000_i64)),
                    ("mpris:artUrl", Value::from("file:///tmp/cover.png")),
                    (
                        "mpris:trackid",
                        Value::from(
                            zbus::zvariant::ObjectPath::try_from("/org/mpd/Track/1")
                                .unwrap_or_else(|e| unreachable!("{e}")),
                        ),
                    ),
                ]),
            ),
            ("Position", owned(Value::from(42_000_000_i64))),
            ("Rate", owned(Value::from(1.5_f64))),
            ("CanGoNext", owned(Value::from(true))),
            ("CanGoPrevious", owned(Value::from(false))),
            ("CanPlay", owned(Value::from(true))),
            ("CanPause", owned(Value::from(true))),
            ("CanSeek", owned(Value::from(true))),
            ("CanControl", owned(Value::from(true))),
            ("LoopStatus", owned(Value::from("Playlist"))),
            ("Shuffle", owned(Value::from(true))),
        ])
    }

    fn sample(position: Duration, sampled_at: Instant) -> PositionSample {
        PositionSample {
            position,
            sampled_at,
            rate: 1.0,
        }
    }

    #[test]
    fn parses_every_player_property() {
        let at = Instant::now();
        let p = parse_player(
            "org.mpris.MediaPlayer2.mpd",
            "Music Player Daemon",
            &full_props(),
            at,
        );
        assert_eq!(p.identity, "Music Player Daemon");
        assert_eq!(p.status, PlaybackStatus::Playing);
        assert_eq!(p.track.title.as_deref(), Some("Song"));
        assert_eq!(p.track.artists, vec!["A".to_string(), "B".to_string()]);
        assert_eq!(p.track.length, Some(Duration::from_secs(180)));
        assert_eq!(p.track.art_url.as_deref(), Some("file:///tmp/cover.png"));
        assert_eq!(
            p.track.id.as_ref().map(|id| id.as_str()),
            Some("/org/mpd/Track/1")
        );
        assert_eq!(p.position.position, Duration::from_secs(42));
        assert_eq!(p.position.sampled_at, at);
        assert!((p.position.rate - 1.5).abs() < f64::EPSILON);
        assert!(p.capabilities.contains(Capabilities::GO_NEXT));
        assert!(!p.capabilities.contains(Capabilities::GO_PREVIOUS));
        assert!(p.capabilities.contains(Capabilities::SEEK));
        assert_eq!(p.loop_status, Some(LoopStatus::Playlist));
        assert_eq!(p.shuffle, Some(true));
    }

    #[test]
    fn optional_properties_stay_absent() {
        let p = parse_player("b", "b", &props(vec![]), Instant::now());
        assert_eq!(p.status, PlaybackStatus::Stopped);
        assert_eq!(p.loop_status, None);
        assert_eq!(p.shuffle, None);
        assert_eq!(p.track, Track::default());
        assert_eq!(p.capabilities, Capabilities::default());
        assert!((p.position.rate - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_title_less_track_falls_back_to_the_players_identity() {
        let p = parse_player("b", "Spotify", &props(vec![]), Instant::now());
        assert_eq!(p.title(), "Spotify");
    }

    #[test]
    fn metadata_tolerates_the_loose_types_players_send() {
        let track = parse_track(&metadata(vec![
            ("xesam:artist", Value::from("Solo")),
            ("mpris:length", Value::from(5_000_000_u64)),
            ("mpris:trackid", Value::from("/org/chromium/Track/9")),
        ]));
        assert_eq!(track.artists, vec!["Solo".to_string()]);
        assert_eq!(track.artists_line().as_deref(), Some("Solo"));
        assert_eq!(track.length, Some(Duration::from_secs(5)));
        assert_eq!(
            track.id.as_ref().map(|id| id.as_str()),
            Some("/org/chromium/Track/9")
        );
    }

    #[test]
    fn no_artists_means_no_artist_line() {
        assert_eq!(Track::default().artists_line(), None);
    }

    #[test]
    fn the_no_track_id_is_no_id() {
        let track = parse_track(&metadata(vec![(
            "mpris:trackid",
            Value::from("/org/mpris/MediaPlayer2/TrackList/NoTrack"),
        )]));
        assert_eq!(track.id, None);
    }

    #[test]
    fn a_negative_or_zero_length_is_unknown() {
        let track = parse_track(&metadata(vec![("mpris:length", Value::from(0_i64))]));
        assert_eq!(track.length, None);
    }

    #[test]
    fn fallback_identity_is_the_first_name_segment() {
        assert_eq!(
            fallback_identity("org.mpris.MediaPlayer2.firefox.instance_1_42"),
            "firefox"
        );
        assert_eq!(fallback_identity("org.mpris.MediaPlayer2.mpv"), "mpv");
        assert_eq!(fallback_identity("weird"), "weird");
    }

    #[test]
    fn unknown_status_strings_are_rejected() {
        assert_eq!(
            PlaybackStatus::parse("Paused"),
            Some(PlaybackStatus::Paused)
        );
        assert_eq!(PlaybackStatus::parse("Buffering"), None);
        assert_eq!(LoopStatus::parse("Track"), Some(LoopStatus::Track));
        assert_eq!(LoopStatus::parse("track"), None);
    }

    #[test]
    fn loop_status_cycles_through_all_three() {
        assert_eq!(LoopStatus::None.next(), LoopStatus::Track);
        assert_eq!(LoopStatus::Track.next(), LoopStatus::Playlist);
        assert_eq!(LoopStatus::Playlist.next(), LoopStatus::None);
    }

    #[test]
    fn loop_status_round_trips_through_its_wire_name() {
        for status in LoopStatus::ALL {
            assert_eq!(LoopStatus::parse(status.as_str()), Some(status));
        }
    }

    #[test]
    fn playerctld_is_not_a_player() {
        assert!(is_player_name("org.mpris.MediaPlayer2.spotify"));
        assert!(!is_player_name("org.mpris.MediaPlayer2.playerctld"));
        assert!(!is_player_name("org.freedesktop.Notifications"));
    }

    #[test]
    fn playing_position_advances_with_time_and_rate() {
        let at = Instant::now();
        let s = PositionSample {
            rate: 2.0,
            ..sample(Duration::from_secs(10), at)
        };
        let now = at + Duration::from_secs(3);
        assert_eq!(
            extrapolate(s, PlaybackStatus::Playing, None, now),
            Duration::from_secs(16)
        );
    }

    #[test]
    fn paused_position_is_frozen() {
        let at = Instant::now();
        let s = sample(Duration::from_secs(10), at);
        let now = at + Duration::from_secs(30);
        assert_eq!(
            extrapolate(s, PlaybackStatus::Paused, None, now),
            Duration::from_secs(10)
        );
    }

    #[test]
    fn position_never_runs_past_the_track() {
        let at = Instant::now();
        let s = sample(Duration::from_secs(170), at);
        let now = at + Duration::from_secs(60);
        assert_eq!(
            extrapolate(
                s,
                PlaybackStatus::Playing,
                Some(Duration::from_secs(180)),
                now
            ),
            Duration::from_secs(180)
        );
    }

    #[test]
    fn a_clock_before_the_sample_does_not_rewind() {
        let now = Instant::now();
        let s = sample(Duration::from_secs(5), now + Duration::from_secs(1));
        assert_eq!(
            extrapolate(s, PlaybackStatus::Playing, None, now),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn a_nonsense_rate_freezes_rather_than_panics() {
        let at = Instant::now();
        for rate in [-1.0, f64::NAN, f64::INFINITY] {
            let s = PositionSample {
                rate,
                ..sample(Duration::from_secs(5), at)
            };
            let now = at + Duration::from_secs(1);
            assert_eq!(
                extrapolate(s, PlaybackStatus::Playing, None, now),
                Duration::from_secs(5)
            );
        }
    }

    fn tracked(owner: &str, player: Option<Player>) -> Tracked {
        Tracked {
            owner: owned_unique(owner),
            player,
        }
    }

    #[test]
    fn a_successful_read_replaces_whatever_was_there() {
        let fresh = Player::test_player("a", PlaybackStatus::Playing);
        let resolved = resolve_read(None, &owned_unique(":1.1"), Some(fresh.clone()));
        assert_eq!(resolved.player, Some(fresh));
    }

    #[test]
    fn a_failed_read_keeps_the_previous_player_under_the_same_owner() {
        let previous = tracked(
            ":1.1",
            Some(Player::test_player("a", PlaybackStatus::Playing)),
        );
        let resolved = resolve_read(Some(&previous), &owned_unique(":1.1"), None);
        assert_eq!(resolved.player, previous.player);
    }

    #[test]
    fn a_failed_read_under_a_new_owner_is_unread() {
        let previous = tracked(
            ":1.1",
            Some(Player::test_player("a", PlaybackStatus::Playing)),
        );
        let resolved = resolve_read(Some(&previous), &owned_unique(":1.2"), None);
        assert_eq!(resolved.player, None);
    }

    #[test]
    fn snapshot_skips_unread_entries() {
        let mut players = BTreeMap::new();
        players.insert("a".to_string(), tracked(":1.1", None));
        players.insert(
            "b".to_string(),
            tracked(
                ":1.2",
                Some(Player::test_player("b", PlaybackStatus::Playing)),
            ),
        );
        assert_eq!(snapshot(&players).len(), 1);
    }
}
