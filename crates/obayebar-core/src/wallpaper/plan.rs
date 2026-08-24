//! Pure wallpaper selection: ordering, dealing to monitors, interval parsing.
//!
//! Everything here is deterministic and IO-free, so it is testable without a
//! filesystem or a compositor. The two shell scripts this replaces each
//! carried their own copy of this logic (`hyprwallp.fish:5-7,27,33` and
//! `hyprrandlock.fish:6-7,17` were character-for-character identical); the
//! background renderer and the lock screen now share one implementation.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// Floor for the rotation interval. A config typo like `interval = "1s"`
/// would otherwise be a decode-and-upload storm rather than a feature.
const MIN_INTERVAL_SECS: u64 = 10;

/// A deterministic PRNG (`SplitMix64`).
///
/// Seeded rather than entropy-driven purely so `build_order` is testable: the
/// same seed must always produce the same order. Not cryptographic and does
/// not need to be — it picks desktop wallpapers.
struct SplitMix64(u64);

impl SplitMix64 {
    const fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..bound`. The modulo bias is irrelevant at the scale of a
    /// wallpaper directory.
    fn below(&mut self, bound: usize) -> usize {
        let picked = u64::try_from(bound)
            .ok()
            .and_then(|b| self.next_u64().checked_rem(b))
            .unwrap_or(0);
        usize::try_from(picked).unwrap_or(0)
    }
}

/// `i + 1` wrapped into `0..len`, written with checked arithmetic so the
/// crate's `arithmetic_side_effects` lint stays quiet. `len` is non-zero at
/// every call site; a zero would yield 0, which is still in range.
fn advance(i: usize, len: usize) -> usize {
    i.saturating_add(1).checked_rem(len).unwrap_or(0)
}

/// Shuffle `files` into a viewing order.
///
/// A stored order plus a cursor is what makes "next wallpaper" meaningful
/// across restarts. The scripts reshuffled from scratch on every invocation,
/// so a wallpaper could repeat immediately on the same monitor and the notion
/// of "next" did not exist.
#[must_use]
pub fn build_order(files: &[PathBuf], seed: u64) -> Vec<PathBuf> {
    let mut out = files.to_vec();
    let mut rng = SplitMix64(seed);
    // Fisher-Yates, back to front.
    let mut i = out.len();
    while let Some(next) = i.checked_sub(1).filter(|n| *n > 0) {
        i = next;
        let j = rng.below(i.saturating_add(1));
        out.swap(i, j);
    }
    out
}

/// One monitor's chosen wallpaper, and where to resume reading the order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WallpaperPlan {
    /// Monitor name -> wallpaper, sorted by monitor so the plan is comparable
    /// and the outcome does not depend on `HashMap` iteration order.
    pub assign: Vec<(String, PathBuf)>,
    /// Where the next pass should start reading `order`.
    pub cursor: usize,
}

/// Deal wallpapers from `order` to `monitors`, starting at `cursor`.
///
/// Wraps when there are fewer wallpapers than monitors, matching the scripts'
/// `(($i - 1) % $WALLPAPERS_COUNT) + 1`. Unlike them it also avoids handing a
/// monitor the wallpaper it already shows — the scripts tried to do this via
/// `! -name "$(basename "$CURRENT_WALL")"`, but `CURRENT_WALL` was never set,
/// so `find ! -name ""` excluded nothing and the filter was dead code.
#[must_use]
pub fn plan_wallpapers<S: std::hash::BuildHasher>(
    monitors: &[String],
    order: &[PathBuf],
    cursor: usize,
    previous: &HashMap<String, PathBuf, S>,
) -> WallpaperPlan {
    if order.is_empty() || monitors.is_empty() {
        return WallpaperPlan {
            assign: Vec::new(),
            cursor,
        };
    }

    let mut names: Vec<&String> = monitors.iter().collect();
    names.sort();
    names.dedup();

    let mut assign = Vec::with_capacity(names.len());
    let mut at = cursor.checked_rem(order.len()).unwrap_or(0);

    for monitor in names {
        let mut chosen = order.get(at).cloned();
        at = advance(at, order.len());
        // Skip a pick that would repeat what this monitor already shows.
        // Bounded to a single skip on purpose: with a pool of one there is
        // nothing else to choose, and an unbounded search could spin.
        if order.len() > 1 && chosen.as_ref() == previous.get(monitor) {
            chosen = order.get(at).cloned();
            at = advance(at, order.len());
        }
        if let Some(path) = chosen {
            assign.push((monitor.clone(), path));
        }
    }

    WallpaperPlan { assign, cursor: at }
}

/// Why an interval string could not be understood.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntervalError(pub String);

impl std::fmt::Display for IntervalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for IntervalError {}

/// Parse a rotation interval such as `"30m"`.
///
/// `Ok(None)` means rotation is switched off — `""`, `"0"`, `"off"`, `"never"`
/// and `"none"` all say so. A bare number is rejected rather than guessed at:
/// `interval = "30"` reads as thirty minutes to a human and would be thirty
/// seconds to any sensible default.
///
/// # Errors
///
/// Returns [`IntervalError`] when the string has no `s`/`m`/`h`/`d` suffix, when
/// the leading digits are not a whole number, or when the result would overflow.
pub fn parse_interval(s: &str) -> Result<Option<Duration>, IntervalError> {
    let text = s.trim().to_ascii_lowercase();
    if matches!(text.as_str(), "" | "0" | "off" | "never" | "none") {
        return Ok(None);
    }

    // `chars().last()` rather than a byte split: a stray multi-byte character
    // would make `split_at(len - 1)` panic on a char boundary.
    let Some(unit) = text.chars().last() else {
        return Ok(None);
    };
    let secs_per: u64 = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3_600,
        'd' => 86_400,
        _ => {
            return Err(IntervalError(format!(
                "{s:?}: expected a trailing unit of s, m, h or d, as in \"30m\""
            )))
        }
    };

    // Not trimmed: the whole string was trimmed already, so any leftover
    // whitespace is *internal* ("30 m") and should be rejected, not absorbed.
    let digits = text.strip_suffix(unit).unwrap_or("");
    let count: u64 = digits.parse().map_err(|_| {
        IntervalError(format!(
            "{s:?}: {digits:?} is not a whole number of {unit} units"
        ))
    })?;
    if count == 0 {
        return Ok(None);
    }
    let secs = count
        .checked_mul(secs_per)
        .ok_or_else(|| IntervalError(format!("{s:?} is too large to be an interval")))?;

    Ok(Some(Duration::from_secs(secs.max(MIN_INTERVAL_SECS))))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    fn monitors(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn order_is_a_permutation_and_seed_stable() {
        let files = paths(&["a", "b", "c", "d", "e"]);
        let first = build_order(&files, 42);
        assert_eq!(
            first,
            build_order(&files, 42),
            "same seed must reorder alike"
        );

        let mut sorted = first;
        sorted.sort();
        let mut expected = files;
        expected.sort();
        assert_eq!(sorted, expected, "shuffle must not add or drop entries");
    }

    #[test]
    fn order_of_one_or_none_is_left_alone() {
        assert!(build_order(&[], 7).is_empty());
        assert_eq!(build_order(&paths(&["only"]), 7), paths(&["only"]));
    }

    #[test]
    fn fewer_wallpapers_than_monitors_wraps() {
        // The behaviour the fish modulo produced: with two wallpapers and
        // three monitors, one wallpaper is reused.
        let order = paths(&["one", "two"]);
        let plan = plan_wallpapers(&monitors(&["a", "b", "c"]), &order, 0, &HashMap::new());
        assert_eq!(plan.assign.len(), 3);
        for (_, path) in &plan.assign {
            assert!(order.contains(path));
        }
    }

    #[test]
    fn single_wallpaper_goes_to_every_monitor() {
        let plan = plan_wallpapers(
            &monitors(&["a", "b"]),
            &paths(&["only"]),
            0,
            &HashMap::new(),
        );
        assert_eq!(
            plan.assign,
            vec![
                ("a".to_string(), PathBuf::from("only")),
                ("b".to_string(), PathBuf::from("only")),
            ]
        );
    }

    #[test]
    fn empty_pool_assigns_nothing_and_keeps_cursor() {
        // Guards the crash the script would have had: `$WALLPAPERS[1]` of an
        // empty list, and a modulo by zero.
        let plan = plan_wallpapers(&monitors(&["a"]), &[], 3, &HashMap::new());
        assert!(plan.assign.is_empty());
        assert_eq!(plan.cursor, 3);
    }

    #[test]
    fn no_monitors_assigns_nothing() {
        let plan = plan_wallpapers(&[], &paths(&["a"]), 0, &HashMap::new());
        assert!(plan.assign.is_empty());
    }

    #[test]
    fn cursor_advances_by_the_monitor_count() {
        let order = paths(&["a", "b", "c", "d", "e"]);
        let plan = plan_wallpapers(&monitors(&["m1", "m2"]), &order, 0, &HashMap::new());
        assert_eq!(plan.assign.len(), 2);
        assert_eq!(plan.cursor, 2, "two monitors consume two entries");
    }

    #[test]
    fn cursor_wraps_past_the_end() {
        let order = paths(&["a", "b", "c"]);
        let plan = plan_wallpapers(&monitors(&["m1", "m2"]), &order, 2, &HashMap::new());
        // Starts at index 2, wraps to 0, ends pointing at 1.
        assert_eq!(plan.cursor, 1);
        assert_eq!(
            plan.assign,
            vec![
                ("m1".to_string(), PathBuf::from("c")),
                ("m2".to_string(), PathBuf::from("a")),
            ]
        );
    }

    #[test]
    fn out_of_range_cursor_is_tolerated() {
        // A stale state file may name a cursor past a now-shorter pool.
        let plan = plan_wallpapers(&monitors(&["m"]), &paths(&["a", "b"]), 99, &HashMap::new());
        assert_eq!(plan.assign.len(), 1);
    }

    #[test]
    fn skips_a_repeat_of_what_the_monitor_shows() {
        let order = paths(&["a", "b"]);
        let mut previous = HashMap::new();
        previous.insert("m".to_string(), PathBuf::from("a"));
        let plan = plan_wallpapers(&monitors(&["m"]), &order, 0, &previous);
        assert_eq!(
            plan.assign,
            vec![("m".to_string(), PathBuf::from("b"))],
            "index 0 is 'a', which is already up, so 'b' is taken instead"
        );
    }

    #[test]
    fn pool_of_one_repeats_rather_than_going_blank() {
        let order = paths(&["only"]);
        let mut previous = HashMap::new();
        previous.insert("m".to_string(), PathBuf::from("only"));
        let plan = plan_wallpapers(&monitors(&["m"]), &order, 0, &previous);
        assert_eq!(plan.assign, vec![("m".to_string(), PathBuf::from("only"))]);
    }

    #[test]
    fn assignments_are_sorted_and_monitors_deduped() {
        let plan = plan_wallpapers(
            &monitors(&["z", "a", "a"]),
            &paths(&["p", "q", "r"]),
            0,
            &HashMap::new(),
        );
        let names: Vec<&str> = plan.assign.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(names, vec!["a", "z"]);
    }

    #[test]
    fn interval_units() {
        assert_eq!(
            parse_interval("45s").unwrap(),
            Some(Duration::from_secs(45))
        );
        assert_eq!(
            parse_interval("30m").unwrap(),
            Some(Duration::from_mins(30))
        );
        assert_eq!(parse_interval("2h").unwrap(), Some(Duration::from_hours(2)));
        assert_eq!(
            parse_interval("1d").unwrap(),
            Some(Duration::from_hours(24))
        );
        assert_eq!(
            parse_interval(" 30M ").unwrap(),
            Some(Duration::from_mins(30))
        );
    }

    #[test]
    fn interval_off_forms() {
        for off in ["", "0", "off", "never", "none", "  ", "OFF"] {
            assert_eq!(parse_interval(off).unwrap(), None, "{off:?} should disable");
        }
        assert_eq!(parse_interval("0m").unwrap(), None);
    }

    #[test]
    fn interval_is_floored_not_honoured_literally() {
        // A 1s rotation is a decode storm; the floor is deliberate.
        assert_eq!(
            parse_interval("1s").unwrap(),
            Some(Duration::from_secs(MIN_INTERVAL_SECS))
        );
    }

    #[test]
    fn interval_rejects_bare_numbers_and_junk() {
        for bad in ["30", "abc", "m", "-5m", "1.5h", "30 m"] {
            assert!(parse_interval(bad).is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn interval_rejects_overflow() {
        assert!(parse_interval("99999999999999999999d").is_err());
        assert!(parse_interval(&format!("{}d", u64::MAX)).is_err());
    }
}
