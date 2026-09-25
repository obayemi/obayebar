//! MPRIS client: every `org.mpris.MediaPlayer2.*` name on the session bus,
//! read into [`Player`] snapshots, plus the commands the media panel sends.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use futures_util::stream::StreamExt;
use futures_util::Stream;
use zbus::names::InterfaceName;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use crate::services::dbus_util::{self, PanelSignal};

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const ROOT_IFACE: &str = "org.mpris.MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";
/// The track id MPRIS reserves for "no track".
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";

/// How often `Position` is re-read while the panel shows a playing player, to
/// correct the drift of the extrapolation.
const RESAMPLE_INTERVAL: Duration = Duration::from_secs(4);

static PANEL: PanelSignal = PanelSignal::new();

/// Toggle from the UI when the media panel opens/closes. Position is only
/// resampled while it is open, since nothing else shows it.
pub fn set_panel_open(open: bool) {
    PANEL.set(open);
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
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "None" => Some(Self::None),
            "Track" => Some(Self::Track),
            "Playlist" => Some(Self::Playlist),
            _ => None,
        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    GoNext,
    GoPrevious,
    Play,
    Pause,
    Seek,
    Control,
}

impl Capability {
    const ALL: [(Self, &'static str); 6] = [
        (Self::GoNext, "CanGoNext"),
        (Self::GoPrevious, "CanGoPrevious"),
        (Self::Play, "CanPlay"),
        (Self::Pause, "CanPause"),
        (Self::Seek, "CanSeek"),
        (Self::Control, "CanControl"),
    ];

    const fn bit(self) -> u8 {
        match self {
            Self::GoNext => 1,
            Self::GoPrevious => 2,
            Self::Play => 4,
            Self::Pause => 8,
            Self::Seek => 16,
            Self::Control => 32,
        }
    }
}

/// The `Can*` properties of a player, as a set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities(u8);

impl Capabilities {
    #[must_use]
    pub const fn with(self, capability: Capability) -> Self {
        Self(self.0 | capability.bit())
    }

    #[must_use]
    pub const fn has(self, capability: Capability) -> bool {
        self.0 & capability.bit() != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Track {
    pub id: Option<OwnedObjectPath>,
    pub title: String,
    pub artists: Vec<String>,
    pub length: Option<Duration>,
    pub art_url: Option<String>,
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
}

/// Build a [`Player`] from the `GetAll` reply of `org.mpris.MediaPlayer2.Player`.
fn parse_player(
    bus_name: &str,
    identity: &str,
    props: &HashMap<String, OwnedValue>,
    sampled_at: Instant,
) -> Player {
    let get = |key: &str| props.get(key);
    let capabilities = Capability::ALL
        .into_iter()
        .filter(|(_, key)| get(key).and_then(as_bool) == Some(true))
        .fold(Capabilities::default(), |caps, (capability, _)| {
            caps.with(capability)
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
        title: get("xesam:title").and_then(as_string).unwrap_or_default(),
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

/// Which player the bar and the panel show.
///
/// The player that most recently started playing wins; before any player has
/// played, the one that appeared last. Cycling from the panel chip pins a
/// player, and the pin holds until some *other* player starts playing.
#[derive(Debug, Default)]
pub struct Selection {
    statuses: HashMap<String, PlaybackStatus>,
    last_started: Option<String>,
    last_seen: Option<String>,
    pinned: Option<String>,
}

impl Selection {
    /// Record a fresh snapshot, noticing appearances and starts.
    pub fn observe(&mut self, players: &[Player]) {
        for player in players {
            let previous = self.statuses.insert(player.bus_name.clone(), player.status);
            if previous.is_none() {
                self.last_seen = Some(player.bus_name.clone());
            }
            let started = player.status == PlaybackStatus::Playing
                && previous != Some(PlaybackStatus::Playing);
            if started {
                self.last_started = Some(player.bus_name.clone());
                if self.pinned.as_ref() != Some(&player.bus_name) {
                    self.pinned = None;
                }
            }
        }
        self.statuses
            .retain(|bus, _| players.iter().any(|p| &p.bus_name == bus));
    }

    #[must_use]
    pub fn active<'a>(&self, players: &'a [Player]) -> Option<&'a Player> {
        [&self.pinned, &self.last_started, &self.last_seen]
            .into_iter()
            .flatten()
            .find_map(|bus| players.iter().find(|p| &p.bus_name == bus))
            .or_else(|| players.iter().find(|p| p.status == PlaybackStatus::Playing))
            .or_else(|| players.first())
    }

    /// Pin the player after the active one, wrapping around.
    pub fn cycle(&mut self, players: &[Player]) {
        let current = self.active(players).map(|p| p.bus_name.as_str());
        let next = players
            .iter()
            .skip_while(|p| Some(p.bus_name.as_str()) != current)
            .nth(1)
            .or_else(|| players.first());
        self.pinned = next.map(|p| p.bus_name.clone());
    }
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

/// A player on the bus, with the unique name that owns it: signals carry the
/// unique sender, never the well-known name.
#[derive(Debug)]
struct Tracked {
    owner: String,
    player: Player,
}

async fn read_player(
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

/// Read `bus_name` into `players`, or drop it when it cannot be read.
async fn track(
    conn: &zbus::Connection,
    players: &mut BTreeMap<String, Tracked>,
    bus_name: &str,
    owner: String,
) {
    let identity = players
        .get(bus_name)
        .filter(|tracked| tracked.owner == owner)
        .map(|tracked| tracked.player.identity.clone());
    match read_player(conn, bus_name, identity).await {
        Some(player) => {
            players.insert(bus_name.to_string(), Tracked { owner, player });
        }
        None => {
            players.remove(bus_name);
        }
    }
}

async fn refresh_where(
    conn: &zbus::Connection,
    players: &mut BTreeMap<String, Tracked>,
    keep: impl Fn(&Tracked) -> bool,
) {
    let stale: Vec<(String, String)> = players
        .iter()
        .filter(|(_, tracked)| keep(tracked))
        .map(|(bus, tracked)| (bus.clone(), tracked.owner.clone()))
        .collect();
    for (bus, owner) in stale {
        track(conn, players, &bus, owner).await;
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
    for name in names.iter().filter(|n| n.starts_with(MPRIS_PREFIX)) {
        let Ok(owner) = dbus.get_name_owner(name.inner().clone()).await else {
            continue;
        };
        track(conn, &mut players, name, owner.to_string()).await;
    }
    players
}

fn snapshot(players: &BTreeMap<String, Tracked>) -> Vec<Player> {
    players.values().map(|t| t.player.clone()).collect()
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
                .any(|t| t.player.status == PlaybackStatus::Playing);
        tokio::select! {
            Some(change) = owner_changes.next() => {
                let Ok(args) = change.args() else { continue };
                let name = args.name().to_string();
                if !name.starts_with(MPRIS_PREFIX) {
                    continue;
                }
                match args.new_owner().as_ref() {
                    Some(owner) => track(conn, &mut players, &name, owner.to_string()).await,
                    None => {
                        players.remove(&name);
                    }
                }
            }
            Some(Ok(signal)) = player_signals.next() => {
                let Some(sender) = signal.header().sender().map(ToString::to_string) else {
                    continue;
                };
                refresh_where(conn, &mut players, |t| t.owner == sender).await;
            }
            () = PANEL.changed() => refresh_where(conn, &mut players, |_| true).await,
            () = tokio::time::sleep(RESAMPLE_INTERVAL), if resample => {
                refresh_where(conn, &mut players, |t| t.player.status == PlaybackStatus::Playing).await;
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

    fn player(bus: &str, status: PlaybackStatus) -> Player {
        Player {
            bus_name: bus.to_string(),
            identity: bus.to_string(),
            status,
            track: Track::default(),
            position: sample(Duration::ZERO, Instant::now()),
            capabilities: Capabilities::default(),
            loop_status: None,
            shuffle: None,
        }
    }

    fn active_bus(selection: &Selection, players: &[Player]) -> Option<String> {
        selection.active(players).map(|p| p.bus_name.clone())
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
        assert_eq!(p.track.title, "Song");
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
        assert!(p.capabilities.has(Capability::GoNext));
        assert!(!p.capabilities.has(Capability::GoPrevious));
        assert!(p.capabilities.has(Capability::Seek));
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
    fn metadata_tolerates_the_loose_types_players_send() {
        let p = parse_player(
            "b",
            "b",
            &props(vec![(
                "Metadata",
                metadata(vec![
                    ("xesam:artist", Value::from("Solo")),
                    ("mpris:length", Value::from(5_000_000_u64)),
                    ("mpris:trackid", Value::from("/org/chromium/Track/9")),
                ]),
            )]),
            Instant::now(),
        );
        assert_eq!(p.track.artists, vec!["Solo".to_string()]);
        assert_eq!(p.track.length, Some(Duration::from_secs(5)));
        assert_eq!(
            p.track.id.as_ref().map(|id| id.as_str()),
            Some("/org/chromium/Track/9")
        );
    }

    #[test]
    fn the_no_track_id_is_no_id() {
        let p = parse_player(
            "b",
            "b",
            &props(vec![(
                "Metadata",
                metadata(vec![(
                    "mpris:trackid",
                    Value::from("/org/mpris/MediaPlayer2/TrackList/NoTrack"),
                )]),
            )]),
            Instant::now(),
        );
        assert_eq!(p.track.id, None);
    }

    #[test]
    fn a_negative_or_zero_length_is_unknown() {
        let p = parse_player(
            "b",
            "b",
            &props(vec![(
                "Metadata",
                metadata(vec![("mpris:length", Value::from(0_i64))]),
            )]),
            Instant::now(),
        );
        assert_eq!(p.track.length, None);
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
        for status in [LoopStatus::None, LoopStatus::Track, LoopStatus::Playlist] {
            assert_eq!(LoopStatus::parse(status.as_str()), Some(status));
        }
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
}
