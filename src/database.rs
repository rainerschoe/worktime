use std::error::Error;
use chrono::Datelike;
use chrono::Timelike;
use chrono::{offset::TimeZone, Local};
#[allow(unused_imports)]
use crate::models::{WorktimeEntry, SpecialDayEntry, SpecialDayType};
use crate::utils::format_chrono_duration;

pub struct Database {
    path: std::path::PathBuf,
    pub rows: Vec<WorktimeEntry>,
    pub special_days: Vec<SpecialDayEntry>,
    file_access_lock: named_lock::NamedLock,
}

impl Database {
    /// Commit an in-progress session.  If an entry with the same start time
    /// already exists it is replaced (auto-save update); otherwise the entry
    /// is inserted in chronological order so the vector stays sorted.
    pub fn commit_worktime(self: &mut Self, entry: WorktimeEntry) {
        if let Some(pos) = self.rows.iter().position(|r| r.start == entry.start) {
            self.rows[pos] = entry;
        } else {
            let pos = self.rows.partition_point(|r| r.start <= entry.start);
            self.rows.insert(pos, entry);
        }
    }

    /// Insert a worktime entry in chronological order (by start time).
    pub fn insert_sorted(self: &mut Self, entry: WorktimeEntry) {
        let pos = self.rows.partition_point(|r| r.start <= entry.start);
        self.rows.insert(pos, entry);
    }

    /// Sort all rows by start time, then merge adjacent/overlapping entries
    /// that have the same `comments` value (or at least one is empty), where
    /// "adjacent" means the gap between them is ≤ `merge_gap_secs` seconds.
    ///
    /// This is called after every mutation to keep the database canonical.
    pub fn normalize(&mut self, merge_gap_secs: i64) {
        // 1. Sort.
        self.rows.sort_by_key(|r| r.start);

        if self.rows.is_empty() {
            return;
        }

        // 2. Merge pass.
        let mut merged: Vec<WorktimeEntry> = Vec::with_capacity(self.rows.len());
        let mut current = self.rows[0].clone();

        for next in self.rows.iter().skip(1) {
            let gap = (next.start - current.end).num_seconds();
            let comments_compatible = current.comments.is_empty()
                || next.comments.is_empty()
                || current.comments == next.comments;

            if gap <= merge_gap_secs && comments_compatible {
                // Merge: extend current to cover next.
                if next.end > current.end {
                    current.end = next.end;
                }
                // Keep the non-empty comment (prefer current's if both set).
                if current.comments.is_empty() && !next.comments.is_empty() {
                    current.comments = next.comments.clone();
                }
            } else {
                merged.push(current);
                current = next.clone();
            }
        }
        merged.push(current);
        self.rows = merged;
    }

    pub fn init(
        path: std::path::PathBuf,
        path_special_days: std::path::PathBuf,
    ) -> Result<Self, String> {
        let file_access_lock = named_lock::NamedLock::create("worktime_file_access").unwrap();
        let mut db = Database {
            path: path.clone(),
            rows: Vec::new(),
            special_days: Vec::new(),
            file_access_lock: file_access_lock,
        };
        // load worktime:
        let mut read_error: Option<csv::Error> = None;
        {
            let _guard = db.file_access_lock.lock();
            let rdr = csv::Reader::from_path(path.clone());
            if let Err(err) = rdr {
                read_error = Some(err);
            } else {
                let mut rdr = rdr.unwrap();
                for result in rdr.deserialize() {
                    // Notice that we need to provide a type hint for automatic
                    // deserialization.
                    if let Err(err) = result {
                        return Err(format!("deserialize {}: {}", path.display(), err));
                    }
                    let record: WorktimeEntry = result.unwrap();
                    db.rows.push(record);
                }
            }
        }
        if let Some(err) = read_error {
            println!("Note: Database could not be fully initialized. Continuing with partially initialized database. Could not read {}: {}", path.display(), err);
            return Ok(db);
        }
        db.rows.sort();
        db.normalize(60); // repair any pre-existing corruption in the CSV

        // load special days:
        let rdr = csv::Reader::from_path(path_special_days.clone());
        if let Err(ref err) = rdr {
            println!("Note: Database could not be fully initialized. Continuing with partially initialized database. Could not read {}: {}", path_special_days.display(), err);
        }
        let mut rdr = rdr.unwrap();
        for result in rdr.deserialize() {
            // Notice that we need to provide a type hint for automatic
            // deserialization.
            if let Err(err) = result {
                return Err(format!(
                    "deserialize {}: {}",
                    path_special_days.display(),
                    err
                ));
            }
            let record: SpecialDayEntry = result.unwrap();
            db.special_days.push(record);
        }
        db.special_days.sort();

        Ok(db)
    }

    pub fn store_file(self: &Self) -> Result<(), Box<dyn Error>> {
        let _guard = self.file_access_lock.lock();
        let mut wtr = csv::Writer::from_path(self.path.clone())?;
        for row in self.rows.iter() {
            wtr.serialize(row)?;
        }
        wtr.flush()?;
        Ok(())
    }

    fn is_in_range<T: core::cmp::PartialOrd>(element: &T, start: &T, end: &T) -> bool {
        element >= start && element < end
    }

    pub fn query_special_days<'a>(
        self: &'a Self,
        range: (
            chrono::DateTime<chrono::offset::Local>,
            chrono::DateTime<chrono::offset::Local>,
        ),
    ) -> impl Iterator<Item = &SpecialDayEntry> + '_ {
        let first = range.0.clone();
        let second = range.1.clone();
        let it = self.special_days.iter().filter(move |x| {
            let naive_day = x
                .day
                .and_time(chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap());
            let day = chrono::Local.from_local_datetime(&naive_day).unwrap();
            Self::is_in_range(&day, &first, &second)
        });
        it
    }

    pub fn query<'a>(
        self: &'a Self,
        range: (
            chrono::DateTime<chrono::offset::Local>,
            chrono::DateTime<chrono::offset::Local>,
        ),
    ) -> impl Iterator<Item = WorktimeEntry> + '_ {
        let first = range.0.clone();
        let second = range.1.clone();
        let it = self
            .rows
            .iter()
            .filter(move |x| {
                let result = Self::is_in_range(&x.start, &first, &second)
                    || Self::is_in_range(&x.end, &first, &second);
                result
            })
            .map::<WorktimeEntry, _>(move |x| {
                if x.start >= first && x.end <= second {
                    // trivial case: entry is completely inside the searched range. Return it:
                    return x.clone();
                }

                let cut_start = if x.start <= first { first } else { x.start };

                let cut_end = if second <= x.end { second } else { x.end };
                WorktimeEntry {
                    start: cut_start,
                    end: cut_end,
                    comments: x.comments.clone(),
                }
            });
        it
    }

    pub fn get_day_bounds(
        time: chrono::DateTime<chrono::offset::Local>,
    ) -> (
        chrono::DateTime<chrono::offset::Local>,
        chrono::DateTime<chrono::offset::Local>,
    ) {
        let date = chrono::NaiveDate::from_ymd_opt(time.year(), time.month(), time.day()).unwrap();

        let time_start = chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap();
        let time_end = chrono::NaiveTime::from_hms_nano_opt(23, 59, 59, 999_999_999).unwrap();

        let day_start = chrono::NaiveDateTime::new(date, time_start);
        let day_end = chrono::NaiveDateTime::new(date, time_end);

        let local_day_start = Local.from_local_datetime(&day_start).unwrap();
        let local_day_end = Local.from_local_datetime(&day_end).unwrap();
        (local_day_start, local_day_end)
    }

    pub fn get_week_bounds(
        time: chrono::DateTime<chrono::offset::Local>,
    ) -> (
        chrono::DateTime<chrono::offset::Local>,
        chrono::DateTime<chrono::offset::Local>,
    ) {
        let week = time.iso_week();

        let date_start =
            chrono::NaiveDate::from_isoywd_opt(week.year(), week.week(), chrono::Weekday::Mon)
                .unwrap();
        let date_end =
            chrono::NaiveDate::from_isoywd_opt(week.year(), week.week(), chrono::Weekday::Sun)
                .unwrap();

        let time_start = chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap();
        let time_end = chrono::NaiveTime::from_hms_nano_opt(23, 59, 59, 999_999_999).unwrap();

        let start = chrono::NaiveDateTime::new(date_start, time_start);
        let end = chrono::NaiveDateTime::new(date_end, time_end);
        let local_start = Local.from_local_datetime(&start).unwrap();
        let local_end = Local.from_local_datetime(&end).unwrap();
        (local_start, local_end)
    }

    pub fn print_simple_summary(self: &Self) {
        let now: chrono::DateTime<chrono::offset::Local> = std::time::SystemTime::now().into();

        let mut previous_entry: Option<WorktimeEntry> = None;
        let mut day_sum = chrono::Duration::seconds(0);
        for entry in self.query(Self::get_day_bounds(now)) {
            if let Some(previous_entry) = previous_entry {
                println!(
                    " {} Pause: {} -> {}",
                    format_chrono_duration(&(entry.start - previous_entry.end)),
                    previous_entry.end.format("%T"),
                    entry.start.format("%T"),
                );
            }
            println!(
                "{} Worked: {} -> {}",
                format_chrono_duration(&entry.duration()),
                entry.start.format("%T"),
                entry.end.format("%T")
            );
            day_sum = day_sum + entry.duration();
            previous_entry = Some(entry);
        }

        let mut week_sum = chrono::Duration::seconds(0);
        for i in self.query(Self::get_week_bounds(now)) {
            week_sum = week_sum + i.duration();
        }

        println!(
            "Current: Day: {}, Week: {}",
            format_chrono_duration(&day_sum),
            format_chrono_duration(&week_sum),
        );
    }

    pub fn print_filler(start: chrono::DateTime<chrono::offset::Local>, end: chrono::DateTime<chrono::offset::Local>, marker: &str) {
        // Print an X for every 15,30,45,00 minute hit between entry.start and entry.end (exclusive)
        let mut mark_time = start;
        // Round up to the next 15-min mark
        let minute = mark_time.time().minute();
        let add_minutes = (15 - (minute % 15)) % 15;
        mark_time = mark_time + chrono::Duration::minutes(add_minutes as i64);
        while mark_time < end {
            println!("| {} {}", mark_time.format("%H:%M"), marker);
            mark_time = mark_time + chrono::Duration::minutes(15);
        }
    }

    pub fn print_vertical_timeline(self: &Self) {
        let now: chrono::DateTime<chrono::offset::Local> = std::time::SystemTime::now().into();

        let mut previous_entry: Option<WorktimeEntry> = None;
        let mut day_sum = chrono::Duration::seconds(0);
        for entry in self.query(Self::get_day_bounds(now)) {
            if let Some(previous_entry) = previous_entry {
                // Grey for filler lines
                print!("\x1b[38;5;250m");
                Self::print_filler(previous_entry.end, entry.start, "");
                print!("\x1b[0m");
                // Green for start working
                println!("\x1b[32m| {} Start working (after {} break)\x1b[0m", entry.start.format("%T"), format_chrono_duration(&(entry.start - previous_entry.end)));
            } else {
                // First entry of the day, no previous entry
                println!("\x1b[32m| {} Start working\x1b[0m", entry.start.format("%T"));
            }
            // Green for X lines
            print!("\x1b[32m");
            Self::print_filler(entry.start, entry.end, "X");
            println!("| {} Stopped working (after {})\x1b[0m", entry.end.format("%T"), format_chrono_duration(&entry.duration()));
            day_sum = day_sum + entry.duration();
            previous_entry = Some(entry);
        }
        print!("\x1b[0m");

        let mut week_sum = chrono::Duration::seconds(0);
        for i in self.query(Self::get_week_bounds(now)) {
            week_sum = week_sum + i.duration();
        }

        println!(
            "Current: Day: {}, Week: {}",
            format_chrono_duration(&day_sum),
            format_chrono_duration(&week_sum),
        );
    }

    /// Like print_vertical_timeline but also shows the current in-progress session.
    ///
    /// # Arguments
    /// * `current_session_start` - Start time of the current in-progress session (if any)
    pub fn print_vertical_timeline_with_current(
        &self,
        current_session_start: Option<chrono::DateTime<chrono::Local>>,
    ) {
        let now: chrono::DateTime<chrono::offset::Local> = std::time::SystemTime::now().into();

        let mut previous_entry: Option<WorktimeEntry> = None;
        let mut day_sum = chrono::Duration::seconds(0);
        for entry in self.query(Self::get_day_bounds(now)) {
            if let Some(previous_entry) = previous_entry {
                print!("\x1b[38;5;250m");
                Self::print_filler(previous_entry.end, entry.start, "");
                print!("\x1b[0m");
                println!("\x1b[32m| {} Start working (after {} break)\x1b[0m", entry.start.format("%T"), format_chrono_duration(&(entry.start - previous_entry.end)));
            } else {
                println!("\x1b[32m| {} Start working\x1b[0m", entry.start.format("%T"));
            }
            print!("\x1b[32m");
            Self::print_filler(entry.start, entry.end, "X");
            println!("| {} Stopped working (after {})\x1b[0m", entry.end.format("%T"), format_chrono_duration(&entry.duration()));
            day_sum = day_sum + entry.duration();
            previous_entry = Some(entry);
        }
        print!("\x1b[0m");

        // Show current in-progress session
        if let Some(session_start) = current_session_start {
            let current_duration = now - session_start;
            // Only show if session started today
            let (day_start, _) = Self::get_day_bounds(now);
            if session_start >= day_start {
                if let Some(prev) = previous_entry {
                    print!("\x1b[38;5;250m");
                    Self::print_filler(prev.end, session_start, "");
                    print!("\x1b[0m");
                    println!("\x1b[32m| {} Start working (after {} break)\x1b[0m",
                        session_start.format("%T"),
                        format_chrono_duration(&(session_start - prev.end)));
                } else {
                    println!("\x1b[32m| {} Start working\x1b[0m", session_start.format("%T"));
                }
                print!("\x1b[33m"); // Yellow for in-progress
                Self::print_filler(session_start, now, "~");
                println!("| {} Working... ({})\x1b[0m", now.format("%T"), format_chrono_duration(&current_duration));
                day_sum = day_sum + current_duration;
            }
        }

        let mut week_sum = chrono::Duration::seconds(0);
        for i in self.query(Self::get_week_bounds(now)) {
            week_sum = week_sum + i.duration();
        }
        // Add current session to week sum too
        if let Some(session_start) = current_session_start {
            let (week_start, _) = Self::get_week_bounds(now);
            if session_start >= week_start {
                week_sum = week_sum + (now - session_start);
            }
        }

        println!(
            "Current: Day: {}, Week: {}",
            format_chrono_duration(&day_sum),
            format_chrono_duration(&week_sum),
        );
    }

    /// Print the vertical timeline from a pre-cloned rows snapshot.
    /// This takes a `Vec` rather than `&self` so the caller can release the
    /// database mutex before calling this (which may do significant I/O).
    pub fn print_timeline_from_rows(
        rows: &[WorktimeEntry],
        now: chrono::DateTime<chrono::Local>,
        current_session_start: Option<chrono::DateTime<chrono::Local>>,
    ) {
        let (day_start, day_end) = Self::get_day_bounds(now);
        let (week_start, week_end) = Self::get_week_bounds(now);

        // Filter to today's entries (mirroring what query() does but on a slice).
        let today: Vec<WorktimeEntry> = rows.iter()
            .filter(|e| {
                (e.start >= day_start && e.start < day_end)
                || (e.end > day_start && e.end <= day_end)
            })
            .map(|e| WorktimeEntry {
                start: e.start.max(day_start),
                end:   e.end.min(day_end),
                comments: e.comments.clone(),
            })
            .collect();

        let mut previous_entry: Option<WorktimeEntry> = None;
        let mut day_sum = chrono::Duration::seconds(0);
        for entry in &today {
            if let Some(ref prev) = previous_entry {
                print!("\x1b[38;5;250m");
                Self::print_filler(prev.end, entry.start, "");
                print!("\x1b[0m");
                println!("\x1b[32m| {} Start working (after {} break)\x1b[0m",
                    entry.start.format("%T"),
                    format_chrono_duration(&(entry.start - prev.end)));
            } else {
                println!("\x1b[32m| {} Start working\x1b[0m", entry.start.format("%T"));
            }
            print!("\x1b[32m");
            Self::print_filler(entry.start, entry.end, "X");
            println!("| {} Stopped working (after {})\x1b[0m",
                entry.end.format("%T"),
                format_chrono_duration(&entry.duration()));
            day_sum = day_sum + entry.duration();
            previous_entry = Some(entry.clone());
        }
        print!("\x1b[0m");

        if let Some(session_start) = current_session_start {
            let current_duration = now - session_start;
            if session_start >= day_start {
                if let Some(ref prev) = previous_entry {
                    print!("\x1b[38;5;250m");
                    Self::print_filler(prev.end, session_start, "");
                    print!("\x1b[0m");
                    println!("\x1b[32m| {} Start working (after {} break)\x1b[0m",
                        session_start.format("%T"),
                        format_chrono_duration(&(session_start - prev.end)));
                } else {
                    println!("\x1b[32m| {} Start working\x1b[0m", session_start.format("%T"));
                }
                print!("\x1b[33m");
                Self::print_filler(session_start, now, "~");
                println!("| {} Working... ({})\x1b[0m",
                    now.format("%T"),
                    format_chrono_duration(&current_duration));
                day_sum = day_sum + current_duration;
            }
        }

        // Week sum from the cloned slice.
        let week_sum: chrono::Duration = rows.iter()
            .filter(|e| {
                (e.start >= week_start && e.start < week_end)
                || (e.end > week_start && e.end <= week_end)
            })
            .map(|e| {
                let s = e.start.max(week_start);
                let t = e.end.min(week_end);
                t - s
            })
            .fold(chrono::Duration::seconds(0), |a, d| a + d);

        // Add current session to week sum if it started this week.
        let week_sum = if let Some(session_start) = current_session_start {
            if session_start >= week_start {
                week_sum + (now - session_start)
            } else {
                week_sum
            }
        } else {
            week_sum
        };

        println!(
            "Current: Day: {}, Week: {}",
            format_chrono_duration(&day_sum),
            format_chrono_duration(&week_sum),
        );
    }

    pub fn get_day_sum(self: &Self, day: chrono::DateTime<chrono::offset::Local>) -> chrono::Duration {
        let mut day_sum = chrono::Duration::seconds(0);
        for entry in self.query(Self::get_day_bounds(day)) {
            day_sum = day_sum + entry.duration();
        }
        day_sum
    }

    pub fn get_day_sums(
        self: &Self,
        num_days: u64,
    ) -> Vec<(chrono::DateTime<chrono::offset::Local>, chrono::Duration)> {
        let mut time: chrono::DateTime<chrono::offset::Local> = std::time::SystemTime::now().into();

        let mut result = Vec::new();
        for _ in 0..num_days {
            let bounds = Self::get_day_bounds(time);
            result.push((bounds.1, self.get_day_sum(time)));
            time -= chrono::Duration::hours(24);
        }

        result
    }

    // TODO: this really requires unittesting...
    pub fn calculate_overtime(
        self: &Self,
        weekly_worktime: chrono::Duration,
        // including start day, excluding end day
        range: (
            chrono::DateTime<chrono::offset::Local>,
            chrono::DateTime<chrono::offset::Local>,
        ),
    ) -> chrono::Duration {
        let (start, end) = range;

        let (start_of_today, _) = Self::get_day_bounds(end);
        let (start_of_calculation, _) = Self::get_day_bounds(start);

        // calculating per day overtime and assuming mostly work only during the week.
        // (Will still calculate weekends correctly, though setting the expectation to work mo-fr)
        // i.e. expect each day mo-f: weekly_hours/5 h of work and expect sa-so 0h of work
        let mut total_hours = chrono::Duration::seconds(0);
        for i in self.query((start_of_calculation, start_of_today)) {
            total_hours = total_hours + i.duration();
        }

        let range = (start_of_today - start_of_calculation).num_days();
        let weeks = range / 7;
        let days = range - weeks * 7;
        let expected_from_whole_weeks = weekly_worktime * weeks.try_into().unwrap();

        // now simulate partial week. We need to do this, as sat and sun do not count as expected
        // work days and we are not aligned with weeks:
        let mut expected_from_partial_weeks = chrono::Duration::seconds(0);
        // NOTE: using checked_add_days here, as it really adds days. a day is not always the same length (e.g. during daylight saving transition)
        let mut day_of_partial_week =
            (start_of_calculation.checked_add_days(
                chrono::naive::Days::new(
                    (7 * weeks) as u64
                )
                ).unwrap()
            ).weekday();
        for _ in 0..days {
            if (day_of_partial_week != chrono::Weekday::Sat)
                && (day_of_partial_week != chrono::Weekday::Sun)
            {
                expected_from_partial_weeks = expected_from_partial_weeks + weekly_worktime / 5;
            }
            day_of_partial_week = day_of_partial_week.succ();
        }

        // now calculate bonus hours received from special days:
        let mut special_days_bonus_time = chrono::Duration::seconds(0);
        for i in self.query_special_days((start_of_calculation, start_of_today)) {
            // weekends cannot have special days:
            match i.day.weekday() {
                chrono::Weekday::Sat => (),
                chrono::Weekday::Sun => (),
                _ => {
                    special_days_bonus_time = special_days_bonus_time + weekly_worktime / 5;
                }
            }
        }

        return total_hours - expected_from_whole_weeks - expected_from_partial_weeks
            + special_days_bonus_time;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_db(rows: Vec<WorktimeEntry>) -> Database {
        Database {
            rows,
            special_days: vec![],
            path: std::path::PathBuf::new(),
            file_access_lock: named_lock::NamedLock::create("dummy_normalize").unwrap(),
        }
    }

    fn entry(start: &str, end: &str) -> WorktimeEntry {
        WorktimeEntry {
            start: start.parse().unwrap(),
            end:   end.parse().unwrap(),
            comments: String::new(),
        }
    }

    fn entry_c(start: &str, end: &str, c: &str) -> WorktimeEntry {
        WorktimeEntry {
            start: start.parse().unwrap(),
            end:   end.parse().unwrap(),
            comments: c.to_string(),
        }
    }

    #[test]
    fn test_normalize_sorts() {
        let mut db = make_db(vec![
            entry("2024-01-02T10:00:00+01:00", "2024-01-02T11:00:00+01:00"),
            entry("2024-01-02T08:00:00+01:00", "2024-01-02T09:00:00+01:00"),
        ]);
        db.normalize(60);
        assert_eq!(db.rows[0].start.format("%H:%M").to_string(), "08:00");
        assert_eq!(db.rows[1].start.format("%H:%M").to_string(), "10:00");
    }

    #[test]
    fn test_normalize_merges_adjacent_within_gap() {
        // Gap of 30s between entries — should merge (threshold 60s).
        let mut db = make_db(vec![
            entry("2024-01-02T08:00:00+01:00", "2024-01-02T09:00:00+01:00"),
            entry("2024-01-02T09:00:30+01:00", "2024-01-02T10:00:00+01:00"),
        ]);
        db.normalize(60);
        assert_eq!(db.rows.len(), 1);
        assert_eq!(db.rows[0].start.format("%H:%M").to_string(), "08:00");
        assert_eq!(db.rows[0].end.format("%H:%M").to_string(), "10:00");
    }

    #[test]
    fn test_normalize_no_merge_beyond_gap() {
        // Gap of 2 minutes — should NOT merge (threshold 60s).
        let mut db = make_db(vec![
            entry("2024-01-02T08:00:00+01:00", "2024-01-02T09:00:00+01:00"),
            entry("2024-01-02T09:02:00+01:00", "2024-01-02T10:00:00+01:00"),
        ]);
        db.normalize(60);
        assert_eq!(db.rows.len(), 2);
    }

    #[test]
    fn test_normalize_merges_overlapping() {
        let mut db = make_db(vec![
            entry("2024-01-02T08:00:00+01:00", "2024-01-02T09:30:00+01:00"),
            entry("2024-01-02T09:00:00+01:00", "2024-01-02T10:00:00+01:00"),
        ]);
        db.normalize(60);
        assert_eq!(db.rows.len(), 1);
        assert_eq!(db.rows[0].end.format("%H:%M").to_string(), "10:00");
    }

    #[test]
    fn test_normalize_no_merge_different_comments() {
        let mut db = make_db(vec![
            entry_c("2024-01-02T08:00:00+01:00", "2024-01-02T09:00:00+01:00", "meeting"),
            entry_c("2024-01-02T09:00:30+01:00", "2024-01-02T10:00:00+01:00", "coding"),
        ]);
        db.normalize(60);
        // Both have non-empty, different comments → not merged.
        assert_eq!(db.rows.len(), 2);
    }

    #[test]
    fn test_normalize_merges_empty_comment_with_nonempty() {
        let mut db = make_db(vec![
            entry_c("2024-01-02T08:00:00+01:00", "2024-01-02T09:00:00+01:00", "meeting"),
            entry_c("2024-01-02T09:00:30+01:00", "2024-01-02T10:00:00+01:00", ""),
        ]);
        db.normalize(60);
        assert_eq!(db.rows.len(), 1);
        assert_eq!(db.rows[0].comments, "meeting");
    }

    #[test]
    fn test_normalize_idempotent() {
        let mut db = make_db(vec![
            entry("2024-01-02T08:00:00+01:00", "2024-01-02T09:00:00+01:00"),
            entry("2024-01-02T10:00:00+01:00", "2024-01-02T11:00:00+01:00"),
        ]);
        db.normalize(60);
        let rows_after_first = db.rows.clone();
        db.normalize(60);
        assert_eq!(db.rows, rows_after_first);
    }


    #[test]
    fn test_overtime_empty_db_empty_range() {
        let db = Database {
            rows: vec![],
            special_days: vec![],
            path: std::path::PathBuf::new(),
            file_access_lock: named_lock::NamedLock::create("dummy").unwrap(),
        };
        let start: chrono::DateTime<chrono::offset::Local> =
            "2023-05-01T00:00:00.00+02:00".parse().unwrap();
        let end: chrono::DateTime<chrono::offset::Local> =
            "2023-05-01T00:00:00.00+02:00".parse().unwrap();
        let weekly_hours = chrono::Duration::hours(40);

        assert_eq!(
            db.calculate_overtime(weekly_hours, (start, end)),
            chrono::Duration::hours(0)
        );
    }

    #[test]
    fn test_overtime_empty_db_valid_range_weekday() {
        let db = Database {
            rows: vec![],
            special_days: vec![],
            path: std::path::PathBuf::new(),
            file_access_lock: named_lock::NamedLock::create("dummy").unwrap(),
        };
        let start: chrono::DateTime<chrono::offset::Local> =
            "2023-05-02T00:00:00.00+02:00".parse().unwrap();
        let end: chrono::DateTime<chrono::offset::Local> =
            "2023-05-04T00:00:00.00+02:00".parse().unwrap();

        let weekly_hours = chrono::Duration::hours(40);

        assert_eq!(
            db.calculate_overtime(weekly_hours, (start, end)),
            chrono::Duration::hours(-16)
        );
    }

    #[test]
    fn test_overtime_empty_db_valid_range_weekend() {
        let db = Database {
            rows: vec![],
            special_days: vec![],
            path: std::path::PathBuf::new(),
            file_access_lock: named_lock::NamedLock::create("dummy").unwrap(),
        };
        let start: chrono::DateTime<chrono::offset::Local> =
            "2023-05-05T00:00:00.00+02:00".parse().unwrap();
        let end: chrono::DateTime<chrono::offset::Local> =
            "2023-05-07T00:00:00.00+02:00".parse().unwrap();

        let weekly_hours = chrono::Duration::hours(40);

        assert_eq!(
            db.calculate_overtime(weekly_hours, (start, end)),
            chrono::Duration::hours(-8)
        );
    }

    #[test]
    fn test_overtime_empty_db_valid_range_only_weekend() {
        let db = Database {
            rows: vec![],
            special_days: vec![],
            path: std::path::PathBuf::new(),
            file_access_lock: named_lock::NamedLock::create("dummy").unwrap(),
        };
        let start: chrono::DateTime<chrono::offset::Local> =
            "2023-05-06T00:00:00.00+02:00".parse().unwrap();
        let end: chrono::DateTime<chrono::offset::Local> =
            "2023-05-08T00:00:00.00+02:00".parse().unwrap();

        let weekly_hours = chrono::Duration::hours(40);

        assert_eq!(
            db.calculate_overtime(weekly_hours, (start, end)),
            chrono::Duration::hours(0)
        );
    }

    #[test]
    fn test_overtime_empty_db_valid_range_full_week() {
        let db = Database {
            rows: vec![],
            special_days: vec![],
            path: std::path::PathBuf::new(),
            file_access_lock: named_lock::NamedLock::create("dummy").unwrap(),
        };
        let start: chrono::DateTime<chrono::offset::Local> =
            "2023-05-17T00:00:00.00+02:00".parse().unwrap();
        let end: chrono::DateTime<chrono::offset::Local> =
            "2023-05-24T00:00:00.00+02:00".parse().unwrap();

        let weekly_hours = chrono::Duration::hours(40);

        assert_eq!(
            db.calculate_overtime(weekly_hours, (start, end)),
            chrono::Duration::hours(-40)
        );
    }

    #[test]
    fn test_overtime_empty_db_valid_range_full_week_not_0_clock() {
        let db = Database {
            rows: vec![],
            special_days: vec![],
            path: std::path::PathBuf::new(),
            file_access_lock: named_lock::NamedLock::create("dummy").unwrap(),
        };
        let start: chrono::DateTime<chrono::offset::Local> =
            "2023-05-17T16:22:12.00+02:00".parse().unwrap();
        let end: chrono::DateTime<chrono::offset::Local> =
            "2023-05-24T02:01:08.00+02:00".parse().unwrap();

        let weekly_hours = chrono::Duration::hours(40);

        assert_eq!(
            db.calculate_overtime(weekly_hours, (start, end)),
            chrono::Duration::hours(-40)
        );
    }

    #[test]
    fn test_overtime_empty_db_valid_range_full_year() {
        let db = Database {
            rows: vec![],
            special_days: vec![],
            path: std::path::PathBuf::new(),
            file_access_lock: named_lock::NamedLock::create("dummy").unwrap(),
        };
        let start: chrono::DateTime<chrono::offset::Local> =
            "2023-01-01T00:00:00.00+01:00".parse().unwrap();
        let end: chrono::DateTime<chrono::offset::Local> =
            "2024-01-01T00:00:00.00+01:00".parse().unwrap();

        let weekly_hours = chrono::Duration::hours(40);

        assert_eq!(
            db.calculate_overtime(weekly_hours, (start, end)),
            chrono::Duration::hours(-2080)
        );
    }

    #[test]
    fn test_overtime_single_entry_worked_sunday() {
        let db = Database {
            rows: vec![WorktimeEntry {
                start: "2023-01-01T00:00:00.00+01:00".parse().unwrap(),
                end: "2023-01-01T01:00:00.00+01:00".parse().unwrap(),
                comments: "".into(),
            }],
            special_days: vec![],
            path: std::path::PathBuf::new(),
            file_access_lock: named_lock::NamedLock::create("dummy").unwrap(),
        };
        let start: chrono::DateTime<chrono::offset::Local> =
            "2023-01-01T00:00:00.00+01:00".parse().unwrap();
        let end: chrono::DateTime<chrono::offset::Local> =
            "2023-01-02T07:00:00.00+01:00".parse().unwrap();

        let weekly_hours = chrono::Duration::hours(40);

        assert_eq!(
            db.calculate_overtime(weekly_hours, (start, end)),
            chrono::Duration::hours(1)
        );
    }

    #[test]
    fn test_overtime_single_entry_worked_monday_full() {
        let db = Database {
            rows: vec![WorktimeEntry {
                start: "2023-01-02T08:00:00.00+01:00".parse().unwrap(),
                end: "2023-01-02T16:00:00.00+01:00".parse().unwrap(),
                comments: "".into(),
            }],
            special_days: vec![],
            path: std::path::PathBuf::new(),
            file_access_lock: named_lock::NamedLock::create("dummy").unwrap(),
        };
        let start: chrono::DateTime<chrono::offset::Local> =
            "2023-01-01T00:00:00.00+01:00".parse().unwrap();
        let end: chrono::DateTime<chrono::offset::Local> =
            "2023-01-03T00:00:00.00+01:00".parse().unwrap();

        let weekly_hours = chrono::Duration::hours(40);

        assert_eq!(
            db.calculate_overtime(weekly_hours, (start, end)),
            chrono::Duration::hours(0)
        );
    }

    #[test]
    fn test_overtime_single_entry_worked_monday_overtime() {
        let db = Database {
            rows: vec![
                WorktimeEntry {
                    start: "2023-01-02T08:00:00.00+01:00".parse().unwrap(),
                    end: "2023-01-02T16:00:00.00+01:00".parse().unwrap(),
                    comments: "".into(),
                },
                WorktimeEntry {
                    start: "2023-01-02T17:00:00.00+01:00".parse().unwrap(),
                    end: "2023-01-02T17:15:00.00+01:00".parse().unwrap(),
                    comments: "".into(),
                },
            ],
            special_days: vec![],
            path: std::path::PathBuf::new(),
            file_access_lock: named_lock::NamedLock::create("dummy").unwrap(),
        };
        let start: chrono::DateTime<chrono::offset::Local> =
            "2023-01-01T00:00:00.00+01:00".parse().unwrap();
        let end: chrono::DateTime<chrono::offset::Local> =
            "2023-01-03T00:00:00.00+01:00".parse().unwrap();

        let weekly_hours = chrono::Duration::hours(40);

        assert_eq!(
            db.calculate_overtime(weekly_hours, (start, end)),
            chrono::Duration::hours(0) + chrono::Duration::minutes(15)
        );
    }

    #[test]
    fn test_overtime_single_entry_worked_out_of_range() {
        let db = Database {
            rows: vec![
                WorktimeEntry {
                    start: "2023-01-02T08:00:00.00+01:00".parse().unwrap(),
                    end: "2023-01-02T16:00:00.00+01:00".parse().unwrap(),
                    comments: "".into(),
                },
                WorktimeEntry {
                    start: "2023-01-03T17:00:00.00+01:00".parse().unwrap(),
                    end: "2023-01-03T17:15:00.00+01:00".parse().unwrap(),
                    comments: "".into(),
                },
            ],
            special_days: vec![],
            path: std::path::PathBuf::new(),
            file_access_lock: named_lock::NamedLock::create("dummy").unwrap(),
        };
        let start: chrono::DateTime<chrono::offset::Local> =
            "2023-01-01T00:00:00.00+01:00".parse().unwrap();
        let end: chrono::DateTime<chrono::offset::Local> =
            "2023-01-03T00:00:00.00+01:00".parse().unwrap();

        let weekly_hours = chrono::Duration::hours(40);

        assert_eq!(
            db.calculate_overtime(weekly_hours, (start, end)),
            chrono::Duration::hours(0)
        );
    }

    #[test]
    fn test_overtime_single_entry_worked_over_midnight_and_a_wohle_year() {
        let db = Database {
            rows: vec![
                WorktimeEntry {
                    start: "2022-01-02T08:00:00.00+01:00".parse().unwrap(),
                    end: "2023-01-01T08:00:00.00+01:00".parse().unwrap(),
                    comments: "".into(),
                },
                WorktimeEntry {
                    start: "2023-01-02T23:00:00.00+01:00".parse().unwrap(),
                    end: "2023-01-03T01:15:00.00+01:00".parse().unwrap(),
                    comments: "".into(),
                },
            ],
            special_days: vec![],
            path: std::path::PathBuf::new(),
            file_access_lock: named_lock::NamedLock::create("dummy").unwrap(),
        };
        let start: chrono::DateTime<chrono::offset::Local> =
            "2023-01-01T00:00:00.00+01:00".parse().unwrap();
        let end: chrono::DateTime<chrono::offset::Local> =
            "2023-01-03T00:00:00.00+01:00".parse().unwrap();

        let weekly_hours = chrono::Duration::hours(40);

        assert_eq!(
            db.calculate_overtime(weekly_hours, (start, end)),
            chrono::Duration::hours(1)
        );
    }

    #[test]
    fn test_overtime_special_day() {
        let db = Database {
            rows: vec![WorktimeEntry {
                start: "2023-01-01T00:00:00.00+01:00".parse().unwrap(),
                end: "2023-01-01T01:00:00.00+01:00".parse().unwrap(),
                comments: "".into(),
            }],
            special_days: vec![SpecialDayEntry {
                day: "2023-01-02".parse().unwrap(),
                day_type: SpecialDayType::Vacation,
            }],
            path: std::path::PathBuf::new(),
            file_access_lock: named_lock::NamedLock::create("dummy").unwrap(),
        };
        let start: chrono::DateTime<chrono::offset::Local> =
            "2023-01-01T00:00:00.00+01:00".parse().unwrap();
        let end: chrono::DateTime<chrono::offset::Local> =
            "2023-01-03T00:00:00.00+01:00".parse().unwrap();

        let weekly_hours = chrono::Duration::hours(40);

        assert_eq!(
            db.calculate_overtime(weekly_hours, (start, end)),
            chrono::Duration::hours(1)
        );
    }

    #[test]
    fn test_overtime_special_day_sunday() {
        let db = Database {
            rows: vec![WorktimeEntry {
                start: "2023-01-01T00:00:00.00+01:00".parse().unwrap(),
                end: "2023-01-01T01:00:00.00+01:00".parse().unwrap(),
                comments: "".into(),
            }],
            special_days: vec![
                SpecialDayEntry {
                    day: "2023-01-01".parse().unwrap(),
                    day_type: SpecialDayType::Vacation,
                },
                SpecialDayEntry {
                    day: "2023-01-02".parse().unwrap(),
                    day_type: SpecialDayType::Vacation,
                },
            ],
            path: std::path::PathBuf::new(),
            file_access_lock: named_lock::NamedLock::create("dummy").unwrap(),
        };
        let start: chrono::DateTime<chrono::offset::Local> =
            "2023-01-01T00:00:00.00+01:00".parse().unwrap();
        let end: chrono::DateTime<chrono::offset::Local> =
            "2023-01-03T00:00:00.00+01:00".parse().unwrap();

        let weekly_hours = chrono::Duration::hours(40);

        assert_eq!(
            db.calculate_overtime(weekly_hours, (start, end)),
            chrono::Duration::hours(1)
        );
    }

    #[test]
    fn test_overtime_special_day_out_range() {
        let db = Database {
            rows: vec![WorktimeEntry {
                start: "2023-01-01T00:00:00.00+01:00".parse().unwrap(),
                end: "2023-01-01T01:00:00.00+01:00".parse().unwrap(),
                comments: "".into(),
            }],
            special_days: vec![
                SpecialDayEntry {
                    day: "2022-12-20".parse().unwrap(),
                    day_type: SpecialDayType::Vacation,
                },
                SpecialDayEntry {
                    day: "2023-01-02".parse().unwrap(),
                    day_type: SpecialDayType::Vacation,
                },
                SpecialDayEntry {
                    day: "2023-01-03".parse().unwrap(),
                    day_type: SpecialDayType::Vacation,
                },
            ],
            path: std::path::PathBuf::new(),
            file_access_lock: named_lock::NamedLock::create("dummy").unwrap(),
        };
        let start: chrono::DateTime<chrono::offset::Local> =
            "2023-01-01T00:00:00.00+01:00".parse().unwrap();
        let end: chrono::DateTime<chrono::offset::Local> =
            "2023-01-03T00:00:00.00+01:00".parse().unwrap();

        let weekly_hours = chrono::Duration::hours(40);

        assert_eq!(
            db.calculate_overtime(weekly_hours, (start, end)),
            chrono::Duration::hours(1)
        );
    }
}