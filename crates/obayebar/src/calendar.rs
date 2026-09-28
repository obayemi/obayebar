//! Month arithmetic behind the calendar panel: which month is shown, and the
//! Monday-first grid of weeks it lays out.

use chrono::{Datelike, Days, Months, NaiveDate};

/// Rows in every month grid. Six weeks fit any month whatever weekday it
/// starts on, and a fixed count keeps the panel height constant.
pub const WEEKS: u8 = 6;

/// A calendar month, identified by its first day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Month {
    first: NaiveDate,
}

/// One row of the grid: an ISO week number and its seven days, Monday first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Week {
    pub number: u32,
    pub days: Vec<Day>,
}

/// One cell of the grid. Days spilling over from the neighbouring months are
/// kept so every row is full, flagged `in_month: false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Day {
    pub date: NaiveDate,
    pub in_month: bool,
}

impl Month {
    #[must_use]
    pub fn containing(date: NaiveDate) -> Self {
        Self {
            first: date.with_day(1).unwrap_or(date),
        }
    }

    #[must_use]
    pub const fn first_day(self) -> NaiveDate {
        self.first
    }

    /// The month `months` away from this one, backwards when negative.
    /// Past the range chrono can represent, it stays on this month.
    #[must_use]
    pub fn shifted(self, months: i32) -> Self {
        let delta = Months::new(months.unsigned_abs());
        let first = if months < 0 {
            self.first.checked_sub_months(delta)
        } else {
            self.first.checked_add_months(delta)
        };
        Self {
            first: first.unwrap_or(self.first),
        }
    }

    #[must_use]
    pub fn weeks(self) -> Vec<Week> {
        let lead = Days::new(u64::from(self.first.weekday().num_days_from_monday()));
        let start = self.first.checked_sub_days(lead).unwrap_or(self.first);
        let days: Vec<Day> = start
            .iter_days()
            .take(usize::from(WEEKS) * 7)
            .map(|date| Day {
                date,
                in_month: Self::containing(date) == self,
            })
            .collect();
        days.chunks(7)
            .map(|days| Week {
                number: days.first().map_or(0, |d| d.date.iso_week().week()),
                days: days.to_vec(),
            })
            .collect()
    }
}

/// Which month the panel shows, as an offset from the current month, paged
/// by buttons and by scrolling.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Pager {
    offset: i32,
    scroll: f32,
}

impl Pager {
    pub const fn page(&mut self, months: i32) {
        self.offset = self.offset.saturating_add(months);
    }

    /// Page by scroll `lines`, upwards going back in time. Fractions
    /// accumulate until a whole line pages one month, so a touchpad pages
    /// steadily and a fast flick never skips months.
    pub fn scroll(&mut self, lines: f32) {
        self.scroll -= lines;
        let months = if self.scroll >= 1.0 {
            1
        } else if self.scroll <= -1.0 {
            -1
        } else {
            return;
        };
        self.page(months);
        self.scroll = 0.0;
    }

    #[must_use]
    pub fn month(self, today: NaiveDate) -> Month {
        Month::containing(today).shifted(self.offset)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::{Month, Pager, WEEKS};
    use chrono::{Datelike, NaiveDate, Weekday};

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("valid date")
    }

    #[test]
    fn a_month_is_named_by_its_first_day() {
        assert_eq!(
            Month::containing(date(2026, 9, 28)).first_day(),
            date(2026, 9, 1)
        );
    }

    #[test]
    fn shifting_crosses_year_boundaries_both_ways() {
        let december = Month::containing(date(2026, 12, 15));
        assert_eq!(december.shifted(1).first_day(), date(2027, 1, 1));
        assert_eq!(december.shifted(-12).first_day(), date(2025, 12, 1));
        assert_eq!(
            Month::containing(date(2027, 1, 31)).shifted(-1).first_day(),
            date(2026, 12, 1)
        );
    }

    #[test]
    fn shifting_by_zero_keeps_the_month() {
        let month = Month::containing(date(2026, 3, 31));
        assert_eq!(month.shifted(0), month);
    }

    #[test]
    fn the_grid_is_six_full_weeks_starting_on_a_monday() {
        let weeks = Month::containing(date(2026, 9, 1)).weeks();
        assert_eq!(weeks.len(), usize::from(WEEKS));
        assert!(weeks.iter().all(|w| w.days.len() == 7));
        assert!(weeks
            .iter()
            .all(|w| w.days.first().map(|d| d.date.weekday()) == Some(Weekday::Mon)));
    }

    #[test]
    fn the_grid_opens_on_the_monday_before_the_first() {
        let weeks = Month::containing(date(2026, 9, 1)).weeks();
        let first = weeks.first().and_then(|w| w.days.first()).expect("a day");
        assert_eq!(first.date, date(2026, 8, 31));
        assert!(!first.in_month);
    }

    #[test]
    fn a_month_starting_on_monday_opens_on_its_first() {
        let weeks = Month::containing(date(2026, 6, 10)).weeks();
        let first = weeks.first().and_then(|w| w.days.first()).expect("a day");
        assert_eq!(first.date, date(2026, 6, 1));
        assert!(first.in_month);
    }

    #[test]
    fn exactly_the_days_of_the_month_are_flagged_in_month() {
        let days: Vec<_> = Month::containing(date(2026, 2, 1))
            .weeks()
            .into_iter()
            .flat_map(|w| w.days)
            .collect();
        let in_month: Vec<_> = days.iter().filter(|d| d.in_month).map(|d| d.date).collect();
        let february: Vec<_> = date(2026, 2, 1).iter_days().take(28).collect();
        assert_eq!(in_month, february);
    }

    #[test]
    fn weeks_carry_their_iso_number() {
        let numbers: Vec<_> = Month::containing(date(2026, 9, 1))
            .weeks()
            .iter()
            .map(|w| w.number)
            .collect();
        assert_eq!(numbers, [36, 37, 38, 39, 40, 41]);
    }

    #[test]
    fn a_week_straddling_new_year_takes_the_iso_number() {
        let weeks = Month::containing(date(2027, 1, 1)).weeks();
        assert_eq!(weeks.first().map(|w| w.number), Some(53));
    }

    #[test]
    fn a_fresh_pager_shows_the_current_month() {
        assert_eq!(
            Pager::default().month(date(2026, 9, 28)).first_day(),
            date(2026, 9, 1)
        );
    }

    #[test]
    fn paging_moves_by_whole_months() {
        let mut pager = Pager::default();
        pager.page(1);
        pager.page(1);
        pager.page(-3);
        assert_eq!(pager.month(date(2026, 9, 28)).first_day(), date(2026, 8, 1));
    }

    #[test]
    fn scrolling_up_goes_back_in_time() {
        let mut pager = Pager::default();
        pager.scroll(1.0);
        assert_eq!(pager.month(date(2026, 9, 28)).first_day(), date(2026, 8, 1));
        pager.scroll(-1.0);
        assert_eq!(pager.month(date(2026, 9, 28)).first_day(), date(2026, 9, 1));
    }

    #[test]
    fn one_scroll_event_pages_at_most_one_month() {
        let mut pager = Pager::default();
        pager.scroll(-3.0);
        assert_eq!(
            pager.month(date(2026, 9, 28)).first_day(),
            date(2026, 10, 1)
        );
        pager.scroll(-0.5);
        assert_eq!(
            pager.month(date(2026, 9, 28)).first_day(),
            date(2026, 10, 1)
        );
    }

    #[test]
    fn fractional_scroll_pages_once_a_line_accumulates() {
        let mut pager = Pager::default();
        pager.scroll(-0.4);
        pager.scroll(-0.4);
        assert_eq!(pager.month(date(2026, 9, 28)).first_day(), date(2026, 9, 1));
        pager.scroll(-0.4);
        assert_eq!(
            pager.month(date(2026, 9, 28)).first_day(),
            date(2026, 10, 1)
        );
    }
}
