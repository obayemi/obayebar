//! Month arithmetic behind the calendar panel: which month is shown, and the
//! Monday-first grid of weeks it lays out.

use chrono::{Datelike, Months, NaiveDate, Weekday};

/// Rows in every month grid. Six weeks fit any month whatever weekday it
/// starts on, and a fixed count keeps the panel height constant.
pub const WEEKS: u8 = 6;

/// Days in a grid row.
const DAYS_PER_WEEK: usize = 7;

/// The weekday the grid, and the panel's header, both start on.
///
/// The single source of the Monday-first decision: `Month::weeks` reads it
/// to find each row's first day, and the panel's header reads it, via
/// `WeekdaySet::ALL.iter(WEEK_START)`, to label the columns in the same
/// order, so the two can never drift apart.
pub const WEEK_START: Weekday = Weekday::Mon;

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

/// What a grid cell represents to the viewer, in the order the panel checks
/// them: today outranks month membership, even for a day spilling over from
/// a neighbouring month.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayKind {
    Today,
    InMonth,
    Spill,
}

impl Day {
    #[must_use]
    pub fn kind(self, today: NaiveDate) -> DayKind {
        if self.date == today {
            DayKind::Today
        } else if self.in_month {
            DayKind::InMonth
        } else {
            DayKind::Spill
        }
    }
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

    /// The [`WEEKS`] rows of the grid, from the [`WEEK_START`] on or before
    /// the first of the month, each carrying its ISO week number.
    #[must_use]
    pub fn weeks(self) -> Vec<Week> {
        let start = self
            .first
            .week(WEEK_START)
            .checked_first_day()
            .unwrap_or(self.first);
        start
            .iter_weeks()
            .take(usize::from(WEEKS))
            .map(|monday| Week {
                number: monday.iso_week().week(),
                days: monday
                    .iter_days()
                    .take(DAYS_PER_WEEK)
                    .map(|date| Day {
                        date,
                        in_month: Self::containing(date) == self,
                    })
                    .collect(),
            })
            .collect()
    }
}

/// A single navigation step: one chevron press, or the whole of one scroll
/// event once its fractions accumulate to a full line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Previous,
    Next,
}

/// Calendar panel input that moves the pager: a chevron press or a scroll
/// event.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Paging {
    Page(Step),
    Scroll(f32),
}

/// Which month the panel shows, as an offset from the current month, paged
/// by buttons and by scrolling.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Pager {
    offset: i32,
    /// Scroll collected toward the next page, in months. Positive goes
    /// forward, negative goes back; a whole line pages one month and resets
    /// this to zero.
    pending: f32,
}

impl Pager {
    pub const fn page(&mut self, step: Step) {
        let months = match step {
            Step::Previous => -1,
            Step::Next => 1,
        };
        self.offset = self.offset.saturating_add(months);
    }

    /// Page by scroll `lines`, upwards going back in time. Fractions
    /// accumulate until a whole line pages one month, so a touchpad pages
    /// steadily and a fast flick never skips months.
    pub fn scroll(&mut self, lines: f32) {
        self.pending -= lines;
        if self.pending.abs() < 1.0 {
            return;
        }
        self.page(if self.pending > 0.0 {
            Step::Next
        } else {
            Step::Previous
        });
        self.pending = 0.0;
    }

    pub fn apply(&mut self, paging: Paging) {
        match paging {
            Paging::Page(step) => self.page(step),
            Paging::Scroll(lines) => self.scroll(lines),
        }
    }

    /// The month shown, counted from the month containing `today`.
    #[must_use]
    pub fn month(self, today: NaiveDate) -> Month {
        Month::containing(today).shifted(self.offset)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::{Day, DayKind, Month, Pager, Paging, Step, WEEKS, WEEK_START};
    use chrono::{Datelike, NaiveDate, Weekday};

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("valid date")
    }

    fn today() -> NaiveDate {
        date(2026, 9, 28)
    }

    fn shown(pager: Pager) -> NaiveDate {
        pager.month(today()).first_day()
    }

    fn first_cell(month: Month) -> Day {
        month
            .weeks()
            .first()
            .and_then(|w| w.days.first())
            .copied()
            .expect("a day")
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
        let first = first_cell(Month::containing(date(2026, 9, 1)));
        assert_eq!(first.date, date(2026, 8, 31));
        assert!(!first.in_month);
    }

    #[test]
    fn a_month_starting_on_monday_opens_on_its_first() {
        let first = first_cell(Month::containing(date(2026, 6, 10)));
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
        assert_eq!(shown(Pager::default()), date(2026, 9, 1));
    }

    #[test]
    fn paging_moves_by_whole_months() {
        let mut pager = Pager::default();
        pager.page(Step::Next);
        pager.page(Step::Next);
        pager.page(Step::Previous);
        pager.page(Step::Previous);
        pager.page(Step::Previous);
        assert_eq!(shown(pager), date(2026, 8, 1));
    }

    #[test]
    fn scrolling_up_goes_back_in_time() {
        let mut pager = Pager::default();
        pager.scroll(1.0);
        assert_eq!(shown(pager), date(2026, 8, 1));
        pager.scroll(-1.0);
        assert_eq!(shown(pager), date(2026, 9, 1));
    }

    #[test]
    fn one_scroll_event_pages_at_most_one_month() {
        let mut pager = Pager::default();
        pager.scroll(-3.0);
        assert_eq!(shown(pager), date(2026, 10, 1));
        pager.scroll(-0.5);
        assert_eq!(shown(pager), date(2026, 10, 1));
    }

    #[test]
    fn fractional_scroll_pages_once_a_line_accumulates() {
        let mut pager = Pager::default();
        pager.scroll(-0.4);
        pager.scroll(-0.4);
        assert_eq!(shown(pager), date(2026, 9, 1));
        pager.scroll(-0.4);
        assert_eq!(shown(pager), date(2026, 10, 1));
    }

    #[test]
    fn apply_dispatches_paging_to_page_and_scroll() {
        let mut pager = Pager::default();
        pager.apply(Paging::Page(Step::Next));
        assert_eq!(shown(pager), date(2026, 10, 1));
        pager.apply(Paging::Scroll(-1.0));
        assert_eq!(shown(pager), date(2026, 11, 1));
    }

    #[test]
    fn the_grid_columns_start_at_week_start_and_run_monday_first() {
        let week = Month::containing(date(2026, 9, 1))
            .weeks()
            .into_iter()
            .next()
            .expect("a week");
        let columns: Vec<_> = week.days.iter().map(|d| d.date.weekday()).collect();
        assert_eq!(columns.first(), Some(&WEEK_START));
        assert_eq!(
            columns,
            [
                Weekday::Mon,
                Weekday::Tue,
                Weekday::Wed,
                Weekday::Thu,
                Weekday::Fri,
                Weekday::Sat,
                Weekday::Sun,
            ]
        );
    }

    #[test]
    fn today_is_flagged_today_even_in_month() {
        let day = Day {
            date: today(),
            in_month: true,
        };
        assert_eq!(day.kind(today()), DayKind::Today);
    }

    #[test]
    fn today_outranks_spilling_over_from_a_neighbouring_month() {
        let day = Day {
            date: today(),
            in_month: false,
        };
        assert_eq!(day.kind(today()), DayKind::Today);
    }

    #[test]
    fn a_month_day_that_is_not_today_is_in_month() {
        let day = Day {
            date: date(2026, 9, 1),
            in_month: true,
        };
        assert_eq!(day.kind(today()), DayKind::InMonth);
    }

    #[test]
    fn a_spill_day_that_is_not_today_is_spill() {
        let day = Day {
            date: date(2026, 8, 31),
            in_month: false,
        };
        assert_eq!(day.kind(today()), DayKind::Spill);
    }
}
