use std::collections::btree_map::Iter;
use std::error::Error;

use chrono::Datelike;
use chrono::Timelike;
use rdev::{listen, Event};
use std::sync::*;
use std::thread;

use signal_hook::{consts::SIGINT, iterator::Signals};

use serde::{Deserialize, Serialize};
use chrono::{offset::TimeZone, Local};


// TODO: file lock: prevent multiple workday instances running on same data store file

#[derive(Serialize, Deserialize)]
struct Config {
    timeout_minutes: u64,
    data_file: String,
    auto_save_interval_seconds: u64,
    weekly_hours: f64,
    cutoff_day_overtime_hours: f64,
    cutoff_datetime: chrono::DateTime<chrono::offset::Local>
}

impl ::std::default::Default for Config {
    fn default() -> Self {
        Self {
            timeout_minutes: 10,
            data_file: "~/.worktime.csv".into(),
            special_day_file: "~/.special_days.csv".into(),
            auto_save_interval_seconds: 30,
            weekly_hours: 40,
            cutoff_day_overtime_hours: 0,
            cutoff_datetime: "2023-05-01T00:00:00.00+02:00".parse().unwrap()
        }
    }
}

fn format_chrono_duration(duration: &chrono::Duration) -> String {
    let sec_total = duration.num_seconds();
    let hours = sec_total / 60 / 60;
    let mins = (sec_total - hours * 60 * 60) / 60;
    let secs = sec_total - hours * 60 * 60 - mins * 60;
    format!("{}h{}m{}s", hours, mins, secs)
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq, PartialOrd, Eq, Ord, Clone)]
struct WorktimeEntry {
    start: chrono::DateTime<chrono::offset::Local>,
    end: chrono::DateTime<chrono::offset::Local>,
    //comments: Vec<String>,
    comments: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq, PartialOrd, Eq, Ord, Clone)]
enum SpecialDayType {
    Vacation,
    Sick,
    Leave
}
#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq, PartialOrd, Eq, Ord, Clone)]
struct SpecialDayEntry {
    day: chrono::naive::NaiveDate,
    day_type: SpecialDayType
}

impl WorktimeEntry
{
    fn duration(self: &Self) -> chrono::Duration
    {
        return self.end-self.start;
    }
}

#[derive(Default)]
struct Database {
    rows: Vec<WorktimeEntry>,
    special_days: Vec<SpecialDayEntry>
}

impl Database {
    /// if last element in database has same start time it is overwritten. otherwise new element is pushed
    fn commit_worktime(self: &mut Self, entry: WorktimeEntry) {
        if self.rows.len() > 0 && self.rows[self.rows.len() - 1].start == entry.start {
            self.rows.pop();
        }
        self.rows.push(entry);
    }

    fn load_file_and_append(
        self: &mut Self,
        path: std::path::PathBuf,
        path_special_days: std::path::PathBuf,
    ) -> Result<(), Box<dyn Error>> {
        // load worktime:
        let mut rdr = csv::Reader::from_path(path)?;
        for result in rdr.deserialize() {
            // Notice that we need to provide a type hint for automatic
            // deserialization.
            let record: WorktimeEntry = result?;
            self.rows.push(record);
        }
        self.rows.sort();

        // load special days:
        let mut rdr = csv::Reader::from_path(path_special_days)?;
        for result in rdr.deserialize() {
            // Notice that we need to provide a type hint for automatic
            // deserialization.
            let record: SpecialDayEntry = result?;
            self.special_days.push(record);
        }
        self.special_days.sort();


        Ok(())
    }

    fn store_file(self: &Self, path: std::path::PathBuf) -> Result<(), Box<dyn Error>> {
        let mut wtr = csv::Writer::from_path(path)?;
        for row in self.rows.iter() {
            wtr.serialize(row)?;
        }
        wtr.flush()?;
        Ok(())
    }

    fn is_in_range<T : core::cmp::PartialOrd>(element: &T, start: &T, end: &T) -> bool
    {
        element >= start && element <= end
    }

    fn query<'a>(
        self: &'a Self,
        range: (chrono::DateTime<chrono::offset::Local>, chrono::DateTime<chrono::offset::Local>)
    ) -> impl Iterator<Item = WorktimeEntry> + '_
    {
        let first = range.0.clone();
        let second = range.1.clone();
        let it = self.rows.iter()
            .filter(move |x|
                {
                    let result = Self::is_in_range(&x.start, &first, &second)
                    ||
                    Self::is_in_range(&x.end, &first, &second);
                    result
                }
            )
            .map::<WorktimeEntry, _>(move |x|
            {
                if x.start >= first && x.end <= second
                {
                    // trivial case: entry is completely inside the searched range. Return it:
                    return x.clone();
                }

                let cut_start = if x.start <= first
                    {
                        first
                    }
                    else 
                    {
                        x.start
                    };

                let cut_end = if second <= x.end
                    {
                        second
                    }
                    else 
                    {
                        x.end
                    };
                WorktimeEntry { start: cut_start, end: cut_end, comments: x.comments.clone() }
            });
        it
    }


    fn get_day_bounds(time: chrono::DateTime<chrono::offset::Local>) -> (chrono::DateTime<chrono::offset::Local>, chrono::DateTime<chrono::offset::Local>)
    {
        let date = chrono::NaiveDate::from_ymd_opt(time.year(), time.month(), time.day()).unwrap();

        let time_start = chrono::NaiveTime::from_hms_opt(0,0,0).unwrap();
        let time_end = chrono::NaiveTime::from_hms_nano_opt(23, 59, 59, 999_999_999).unwrap();

        let day_start = chrono::NaiveDateTime::new(date,time_start);
        let day_end = chrono::NaiveDateTime::new(date,time_end);

        let local_day_start = Local.from_local_datetime(&day_start).unwrap();
        let local_day_end = Local.from_local_datetime(&day_end).unwrap();
        (local_day_start, local_day_end)
    }

    fn get_week_bounds(time: chrono::DateTime<chrono::offset::Local>) -> (chrono::DateTime<chrono::offset::Local>, chrono::DateTime<chrono::offset::Local>)
    {
        let week = time.iso_week();

        let date_start = chrono::NaiveDate::from_isoywd_opt(week.year(), week.week(), chrono::Weekday::Mon).unwrap();
        let date_end = chrono::NaiveDate::from_isoywd_opt(week.year(), week.week(), chrono::Weekday::Sun).unwrap();

        let time_start = chrono::NaiveTime::from_hms_opt(0,0,0).unwrap();
        let time_end = chrono::NaiveTime::from_hms_nano_opt(23, 59, 59, 999_999_999).unwrap();

        let start = chrono::NaiveDateTime::new(date_start,time_start);
        let end = chrono::NaiveDateTime::new(date_end,time_end);
        let local_start = Local.from_local_datetime(&start).unwrap();
        let local_end = Local.from_local_datetime(&end).unwrap();
        (local_start, local_end)
    }

    fn print_simple_summary(self: &Self)
    {
        let now: chrono::DateTime<chrono::offset::Local> = std::time::SystemTime::now().into();

        let mut previous_entry: Option<WorktimeEntry> = None;
        let mut day_sum = chrono::Duration::seconds(0);
        for entry in 
            self.query(Self::get_day_bounds(now))
        {
            if let Some(previous_entry) = previous_entry {
                println!(
                    "  Pause: {}",
                    format_chrono_duration(&(entry.start - previous_entry.end))
                );
            }
            println!(
                " Start: {} End: {} ({}) {:#?}",
                entry.start.format("%Y-%m-%d %T"),
                entry.end.format("%T"),
                format_chrono_duration(&entry.duration()),
                entry.comments
            );
            day_sum = day_sum + entry.duration();
            previous_entry = Some(entry);
        }

        let mut week_sum = chrono::Duration::seconds(0);
        for i in 
            self.query(Self::get_week_bounds(now))
        {
            week_sum = week_sum + i.duration();
        }

        println!(
            "Current: Day: {}, Week: {}",
            format_chrono_duration(&day_sum),
            format_chrono_duration(&week_sum),
        );
    }

    // TODO: this really requires unittesting...
    fn calculate_overtime(
        self: &Self
        weekly_worktime: chrono::Duration,
        start_datetime: chrono::DateTime<chrono::offset::Local>
        ) -> chrono::Duration
    {
        let now: chrono::DateTime<chrono::offset::Local> = std::time::SystemTime::now().into();
        let (start_of_today, _) = Self::get_day_bounds(now);
        let (start_of_calculation, _) = Self::get_day_bounds(start_datetime);

        // calculating per day overtime and assuming mostly work only during the week.
        // (Will still calculate weekends correctly, though setting the expectation to work mo-fr)
        // i.e. expect each day mo-f: weekly_hours/5 h of work and expect sa-so 0h of work
        let mut total_hours = chrono::Duration::seconds(0);
        for i in 
            self.query( (start_of_calculation, start_of_today) )
        {
            total_hours = total_hours + i.duration();
        }

        let range = (start_of_today - start_of_calculation).num_days;
        let weeks = range/7;
        let days = range-weeks*7;
        let expected_from_whole_weeks = weeks * weekly_worktime;

        // now simulate partial week. We need to do this, as sat and sun do not count and we are
        // not aligned with weeks:
        let mut expected_from_partial_weeks = chrono::Duration::seconds(0);
        let mut day_of_partial_week = (start_of_calculation + chrono::Duration::days(1) * 7 * weeks).weekday();
        for i in 0..days
        {
            if 
                (day_of_partial_week != chrono::Weekday::Sat) &&
                (day_of_partial_week != chrono::Weekday::Sun)
            {

                expected_from_partial_weeks = expected_from_partial_weeks + weekly_worktime / 5;
            }
            day_of_partial_week = day_of_partial_week.succ();
        }

        return total_hours - expected_from_whole_weeks - expected_from_partial_weeks;
    }

    // idea:
    // query(start time, end time) -> iter/List of worktime entries
    // split(time, WorktimeEntry) -> (before_time=Option<WorktimeEntry>, after_time=Option<WorktimeEntry>)
    // sum(iter/List of worktime Entries) -> (List(dailySum(Day, sum_hours)), List(weekly_sum(Day,hours)), ..., TotalSumHours)
    // split needed in query (to cut start and end entries) and also in sum, to cut day start/end entries
    fn print_summary(
        self: &Self,
        first: chrono::DateTime<chrono::offset::Local>,
        second: chrono::DateTime<chrono::offset::Local>,
    ) {
        let mut previous_entry: Option<&WorktimeEntry> = None;
        let mut daily_sum = chrono::Duration::seconds(0);
        let mut weekly_sum = chrono::Duration::seconds(0);
        let mut monthly_sum = chrono::Duration::seconds(0);
        let mut total_sum = chrono::Duration::seconds(0);

        let iter_rows = self
            .rows
            .iter()
            .filter(|x| x.start >= first && x.end <= second);

        for entry in iter_rows {
            let duration = entry.end - entry.start;

            daily_sum = daily_sum + duration;
            weekly_sum = weekly_sum + duration;
            monthly_sum = monthly_sum + duration;
            total_sum = total_sum + duration;

            if let Some(previous) = previous_entry {
                if previous.start.day() != entry.start.day() {
                    // new day
                    let day_format = entry.start.format("%Y-%m-%d %a");
                    println!("{}: {}", day_format, format_chrono_duration(&daily_sum));
                    daily_sum = chrono::Duration::seconds(0);
                }
                if previous.start.weekday() != entry.start.weekday()
                    && previous.start.weekday() == chrono::Weekday::Sun
                {
                    // new week
                    println!("week: {}", format_chrono_duration(&weekly_sum));
                    weekly_sum = chrono::Duration::seconds(0);
                }
                if previous.start.month() != entry.start.month() {
                    // new month
                    println!("month: {}", format_chrono_duration(&monthly_sum));
                    monthly_sum = chrono::Duration::seconds(0);
                }
            }
            if let Some(previous_entry) = previous_entry {
                println!(
                    "  Pause: {}",
                    format_chrono_duration(&(entry.start - previous_entry.end))
                );
            }
            println!(
                " Start: {} End: {} ({}) {:#?}",
                entry.start.format("%Y-%m-%d %T"),
                entry.end.format("%T"),
                format_chrono_duration(&duration),
                entry.comments
            );
            previous_entry = Some(entry);
        }

        println!(
            "Current: Day: {}, Week: {}, Month: {}",
            format_chrono_duration(&daily_sum),
            format_chrono_duration(&weekly_sum),
            format_chrono_duration(&monthly_sum)
        );
    }
}

enum EventType {
    Activity(Event),
    Comment(String), // TODO: implement comment user interface
    Commit,
}

struct ActivityRecorder {
    database: Arc<Mutex<Database>>,
    last_event_time: chrono::DateTime<chrono::offset::Local>,
    last_start_time: chrono::DateTime<chrono::offset::Local>,
    comments: String,
    timeouts_minutes: u64,
}

impl ActivityRecorder {
    fn new(database: Arc<Mutex<Database>>, timeout_minutes: u64) -> Self {
        let now = std::time::SystemTime::now().into();
        ActivityRecorder {
            database: database,
            last_event_time: now,
            last_start_time: now,
            comments: "".into(),
            timeouts_minutes: timeout_minutes,
        }
    }

    fn handle_event(self: &mut Self, event: EventType) {
        match event {
            EventType::Activity(event) => {
                let event_time: chrono::DateTime<chrono::offset::Local> = event.time.into();
                let time_since_last_activity = event_time - self.last_event_time;
                if time_since_last_activity
                    > chrono::Duration::minutes(self.timeouts_minutes as i64)
                {
                    self.database
                        .lock()
                        .unwrap()
                        .commit_worktime(WorktimeEntry {
                            start: self.last_start_time,
                            end: self.last_event_time,
                            comments: self.comments.clone(),
                        });
                    self.last_start_time = event_time;
                    self.comments.clear();
                }
                self.last_event_time = event_time;
            }
            EventType::Comment(comment) => {
                self.comments.push_str(&comment);
            }
            EventType::Commit => {
                self.database
                    .lock()
                    .unwrap()
                    .commit_worktime(WorktimeEntry {
                        start: self.last_start_time,
                        end: self.last_event_time,
                        comments: self.comments.clone(),
                    });
            }
        }
    }
}

fn main() {
    let cfg: Config = confy::load("worktime", None).unwrap();

    let database = Arc::new(Mutex::new(Database::default()));

    let data_path = expanduser::expanduser(cfg.data_file.as_str()).unwrap();
    println!("Using data file {}", data_path.display());

    if let Err(_) = database
        .lock()
        .unwrap()
        .load_file_and_append(data_path.clone().into())
    {
        println!(
            "Note: Data file {} could not be loaded. Not retrieving any historic worktime data...",
            data_path.display()
        );
    }

    let activity_recorder = Arc::new(Mutex::new(ActivityRecorder::new(
        database.clone(),
        cfg.timeout_minutes,
    )));

    // Mutex used to syncronize auto-save with sigint handling (in order to avoid terminating in the middle of writing into file)
    // Note: termination is a bit messy right now, as I do not cleanly join all threads, so I need hacks like this
    let file_mutex_signal = Arc::new(Mutex::new(()));
    let file_mutex_auto_save = file_mutex_signal.clone();

    // monitor signals:
    let activity_recorder_signals = activity_recorder.clone();
    let database_signals = database.clone();
    let data_path_signals = data_path.clone();
    thread::spawn(move || {
        let mut signals = Signals::new(&[SIGINT]).unwrap();
        for sig in signals.forever() {
            println!("Received signal {:?}", sig);
            activity_recorder_signals
                .lock()
                .unwrap()
                .handle_event(EventType::Commit);
            println!("Saving worktimes into data file...");
            database_signals
                .lock()
                .unwrap()
                .store_file(data_path_signals.into())
                .unwrap();
            // lock mutex here, which prevents any auto-save to try saving while we exit
            let _lock = file_mutex_signal.lock();
            std::process::exit(0);
        }
    });

    // monitor mouse/key events:
    let activity_recorder_activity = activity_recorder.clone();
    thread::spawn(move || {
        if let Err(error) = listen(move |event| {
            activity_recorder_activity
                .lock()
                .unwrap()
                .handle_event(EventType::Activity(event));
        }) {
            println!("Error: {:?}", error)
        }
    });

    // auto-save:
    let database_autosave = database.clone();
    thread::spawn(move || loop {
        thread::sleep(std::time::Duration::from_secs(
            cfg.auto_save_interval_seconds,
        ));
        println!("Auto-Save");
        let _lock = file_mutex_auto_save.lock();
        database_autosave
            .lock()
            .unwrap()
            .store_file(data_path.clone().into())
            .unwrap();
    });

    // monitor terminal input:
    // TODO

    loop {
        thread::sleep(std::time::Duration::from_secs(2));
        //thread::sleep(chrono::Duration::seconds(2));
        let now: chrono::DateTime<chrono::offset::Local> = std::time::SystemTime::now().into();
        let day_start = now.with_hour(0).unwrap().with_minute(0).unwrap();
        let day_end = now.with_hour(23).unwrap().with_minute(59).unwrap();
        //.with_seconds(0).with_nanoseconds(0);
        activity_recorder
            .lock()
            .unwrap()
            .handle_event(EventType::Commit);
        println!("---");
        //database.lock().unwrap().print_summary(day_start, day_end);
        database.lock().unwrap().print_simple_summary();
    }
}
