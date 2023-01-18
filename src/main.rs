use std::time::SystemTime;
use std::error::Error;

use chrono::{Datelike, DateTime};
use rdev::{listen, Event};
use std::sync::*;
use std::thread;
use chrono::Timelike;

use signal_hook::{consts::SIGINT, iterator::Signals};

fn format_time(time: &SystemTime) -> String
{
    let datetime:  chrono::DateTime< chrono::offset::Local> = (*time).into();
    format!("{}", datetime.format("%Y-%m-%d %T"))
}

fn format_chrono_duration(duration: &chrono::Duration) -> String
{

    let sec_total = duration.num_seconds();
    let hours = sec_total / 60 /60;
    let mins = (sec_total - hours*60*60)/60;
    let secs = sec_total - hours*60*60 - mins*60;
    format!("{}h{}m{}s", hours, mins, secs)
}

fn format_duration(duration: &std::time::Duration) -> String
{

    let sec_total = duration.as_secs();
    let hours = sec_total / 60 /60;
    let mins = (sec_total - hours*60*60)/60;
    let secs = sec_total - hours*60*60 - mins*60;
    format!("{}h{}m{}s", hours, mins, secs)
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq, PartialOrd, Eq, Ord)]
struct WorktimeEntry
{
    start: chrono::DateTime< chrono::offset::Local>,
    end: chrono::DateTime< chrono::offset::Local>,
    //comments: Vec<String>,
    comments: String
}

#[derive(Default)]
struct Database
{
    rows: Vec<WorktimeEntry>,
}

impl Database 
{
    /// if last element in database hase same start time it is overwritten. otherwise new element is pushed
    fn commit_worktime(self : &mut Self, entry: WorktimeEntry)
    {
        if self.rows.len() > 0 && self.rows[self.rows.len()-1].start == entry.start
        {
            self.rows.pop();
        }
        self.rows.push(entry);
    }

    fn load_file_and_append(self: &mut Self, path: String) -> Result<(), Box<dyn Error>>
    {
        let mut rdr = csv::Reader::from_path(path)?;
        for result in rdr.deserialize() {
            // Notice that we need to provide a type hint for automatic
            // deserialization.
            let record: WorktimeEntry = result?;
            self.rows.push(record);
        }
        self.rows.sort();
        Ok(())
    }

    fn store_file(self: & Self, path: String) -> Result<(), Box<dyn Error>>
    {
        let mut wtr = csv::Writer::from_path(path)?;
        for row in self.rows.iter()
        {
            wtr.serialize(row);
        }
        wtr.flush()?;
        Ok(())
    }

    fn print_summary(self: &Self, first: chrono::DateTime<chrono::offset::Local>, second: chrono::DateTime<chrono::offset::Local>)
    {

        let mut previous_entry : Option<&WorktimeEntry> = None;
        let mut daily_sum   = chrono::Duration::seconds(0);
        let mut weekly_sum  = chrono::Duration::seconds(0);
        let mut monthly_sum = chrono::Duration::seconds(0);
        let mut total_sum   = chrono::Duration::seconds(0);

        let iter_rows = self.rows.iter().filter(|x| {x.start >= first && x.end <= second} );

        for entry in iter_rows
        {
            let duration = entry.end - entry.start;

            daily_sum = daily_sum + duration;
            weekly_sum = weekly_sum + duration;
            monthly_sum = monthly_sum + duration;
            total_sum = total_sum + duration;

            if let Some(previous) = previous_entry
            {
                if previous.start.day() != entry.start.day()
                {
                    // new day
                    let day_format = entry.start.format("%Y-%m-%d %a");
                    println!("{}: {}", day_format, format_chrono_duration(&daily_sum));
                    daily_sum = chrono::Duration::seconds(0);
                }
                if previous.start.weekday() != entry.start.weekday() && previous.start.weekday() == chrono::Weekday::Sun
                {
                    // new week
                    println!("week: {}", format_chrono_duration(&weekly_sum));
                    weekly_sum = chrono::Duration::seconds(0);
                }
                if previous.start.month() != entry.start.month()
                {
                    // new month
                    println!("month: {}", format_chrono_duration(&monthly_sum));
                    monthly_sum = chrono::Duration::seconds(0);
                }
            }
            println!(" Start: {} End: {} ({}) // {:#?}", entry.start.format("%Y-%m-%d %T"), entry.end.format("%T"), format_chrono_duration(&duration), entry.comments );
            previous_entry = Some(entry);
        }
        
        println!("Current: Day: {}, Week: {}, Month: {}", format_chrono_duration(&daily_sum), format_chrono_duration(&weekly_sum), format_chrono_duration(&monthly_sum));


    }
}


enum EventType
{
    Activity(Event),
    Comment(String),
    Commit
}

struct ActivityRecorder
{
    database: Arc<Mutex<Database>>,
    last_event_time : chrono::DateTime< chrono::offset::Local>,
    last_start_time : chrono::DateTime< chrono::offset::Local>,
    comments: Vec<String>
}

impl ActivityRecorder
{
    fn new(database : Arc<Mutex<Database>>) -> Self
    {
        let now = std::time::SystemTime::now().into();
        ActivityRecorder { database: database, last_event_time: now, last_start_time: now, comments: Vec::new() }
    }

    fn handle_event(self: &mut Self, event: EventType)
    {
        match event
        {
            EventType::Activity(event) =>
            {
                let event_time : chrono::DateTime<chrono::offset::Local> = event.time.into();
                let time_since_last_activity = event_time - self.last_event_time;
                if time_since_last_activity > chrono::Duration::seconds(10)
                {
                    self.database.lock().unwrap().commit_worktime(WorktimeEntry { start: self.last_start_time, end: self.last_event_time, comments: "z,z".into() });
                    self.last_start_time = event_time;
                    self.comments.clear();
                }
                self.last_event_time = event_time;
            }
            EventType::Comment(comment) =>
            {
                self.comments.push(comment);
            }
            EventType::Commit =>
            {
                self.database.lock().unwrap().commit_worktime(WorktimeEntry { start: self.last_start_time, end: self.last_event_time, comments: "z,z".into() });
                //self.database.lock().unwrap().commit_worktime(WorktimeEntry { start: self.last_start_time, end: self.last_event_time, comments: self.comments.clone() });
            }
        }
    }
}

fn main() {
    println!("Hello, world!");
    println!("Started working at {}", format_time(&SystemTime::now()));

    let database = Arc::new(Mutex::new(Database::default()));

    database.lock().unwrap().load_file_and_append("/home/rschoe/worktime.csv".into()).unwrap();

    let activity_recorder = Arc::new(Mutex::new(ActivityRecorder::new(database.clone())));

    // monitor signals:
    let activity_recorder_signals = activity_recorder.clone();
    let database_signals = database.clone();
    thread::spawn(move || {
        let mut signals = Signals::new(&[SIGINT]).unwrap();
        for sig in signals.forever() {
            println!("Received signal {:?}", sig);
            activity_recorder_signals.lock().unwrap().handle_event(EventType::Commit);
            database_signals.lock().unwrap().store_file("/home/rschoe/worktime.csv".into()).unwrap();
            std::process::exit(0);
        }
    });

    // monitor mouse/key events:
    let activity_recorder_activity = activity_recorder.clone();
    thread::spawn(move || {
        if let Err(error) = listen(move
            |event|
            {
            activity_recorder_activity.lock().unwrap().handle_event(EventType::Activity(event));
            }
        ) {
            println!("Error: {:?}", error)
        }
    });

    // monitor terminal input:
    // TODO
    loop
    {
        thread::sleep(std::time::Duration::from_secs(2));
        //thread::sleep(chrono::Duration::seconds(2));
        let now: chrono::DateTime<chrono::offset::Local> = std::time::SystemTime::now().into();
        let day_start = now.with_hour(0).unwrap().with_minute(0).unwrap();
        let day_end = now.with_hour(23).unwrap().with_minute(59).unwrap();
        //.with_seconds(0).with_nanoseconds(0);
        activity_recorder.lock().unwrap().handle_event(EventType::Commit);
        println!("---");
        database.lock().unwrap().print_summary(day_start, day_end);
    }
}
