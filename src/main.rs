use std::time::SystemTime;
use std::error::Error;

use chrono::Datelike;
use rdev::{listen, Event};

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
    comments: Vec<String>,
}

struct Database
{
    rows: Vec<WorktimeEntry>,
}

impl Database 
{

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

    fn print_summary(self: &Self, first: chrono::DateTime<chrono::offset::Local>, second: chrono::DateTime<chrono::offset::Local>, partial_entry: Option<chrono::DateTime<chrono::offset::Local>>)
    {
        let mut previous_entry : Option<&WorktimeEntry> = None;
        let mut daily_sum   = chrono::Duration::days(0);
        let mut weekly_sum  = chrono::Duration::days(0);
        let mut monthly_sum = chrono::Duration::days(0);
        let mut total_sum   = chrono::Duration::days(0);

        let element_op = |entry: &WorktimeEntry|
        {
            let duration = entry.end - entry.start;

            daily_sum.checked_add(&duration).unwrap();
            weekly_sum.checked_add(&duration).unwrap();
            monthly_sum.checked_add(&duration).unwrap();
            total_sum.checked_add(&duration).unwrap();

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
            println!(" Start: {} End: {} ({}) // {}", entry.start.format("%Y-%m-%d %T"), entry.end.format("%T"), format_chrono_duration(&duration), entry.comments );
            previous_entry = Some(entry);
        };

        for entry in self.rows.iter().filter(|x| {x.start >= first && x.end <= second} )
        {
            element_op(entry);
        }
        if let Some(partial_start) = partial_entry
        {
            element_op(&WorktimeEntry { start: partial_start, end: std::time::SystemTime::now().into() , comments: vec!["...ongoing".into()] });
        }
    }
}

//  2023-01-09 22:38:44; 2023-01-09 22:39:01; Comments

fn callback(event: Event, store: &mut Store) {
    if let Ok(time_since_last_activity) = event.time.duration_since(store.last_event_time)
    {
        if time_since_last_activity > std::time::Duration::from_secs(10)
        //if time_since_last_activity > std::time::Duration::from_secs(10*60)
        {
            if let Ok(worked_time) = store.last_event_time.duration_since(store.last_start_time)
            {
                println!(" Worked: {}", format_duration(&worked_time));
            }
            else 
            {

                println!(" Worked: 0h0m0s");
            }
            println!("Stopped working at {}", format_time(& store.last_event_time));
            println!(" Pause: {}", format_duration(&time_since_last_activity));
            println!("Started working at {}", format_time(& event.time));
            store.last_start_time = event.time;
        }
    }
    store.last_event_time = event.time;
    //println!("My callback {:?}", event);
 //   match event.name {
 //       Some(string) => println!("User wrote {:?}", string),
 //       None => (),
 //   }
}

struct Store
{
    last_event_time : SystemTime,
    last_start_time : SystemTime
}

fn main() {
    println!("Hello, world!");
    println!("Started working at {}", format_time(&SystemTime::now()));
    // This will block.
    let mut store = Store{
        last_event_time: SystemTime::now(),
        last_start_time: SystemTime::now()
    };
    if let Err(error) = listen(move
        |event|
        {
        callback(event, &mut store)
        }
    ) {
        println!("Error: {:?}", error)
    }
}
