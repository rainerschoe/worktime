use std::time::SystemTime;

use rdev::{listen, Event};

fn format_time(time: &SystemTime) -> String
{
    let datetime:  chrono::DateTime< chrono::offset::Local> = (*time).into();
    format!("{}", datetime.format("%Y-%m-%d %T"))
}

fn format_duration(duration: &std::time::Duration) -> String
{

    let sec_total = duration.as_secs();
    let hours = sec_total / 60 /60;
    let mins = (sec_total - hours*60*60)/60;
    let secs = sec_total - hours*60*60 - mins*60;
    format!("{}h{}m{}s", hours, mins, secs)
}

fn callback(event: Event, store: &mut Store) {
    if let Ok(time_since_last_activity) = event.time.duration_since(store.last_event_time)
    {
        //if time_since_last_activity > std::time::Duration::from_secs(10)
        if time_since_last_activity > std::time::Duration::from_secs(10*60)
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
