use std::sync::*;
use std::thread;
use rdev::listen;
use signal_hook::{consts::SIGINT, iterator::Signals};
use crate::config::Config;
use crate::database::Database;
use crate::idle_detection::{ActivityRecorder, EventType};
use crate::utils::format_chrono_duration;

pub fn run_interactive_monitoring(database: Arc<Mutex<Database>>, cfg: &Config) {
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
    thread::spawn(move || {
        let mut signals = Signals::new(&[SIGINT]).unwrap();
        for sig in signals.forever() {
            println!("Received signal {:?}", sig);
            activity_recorder_signals
                .lock()
                .unwrap()
                .handle_event(EventType::Commit);
            println!("Saving worktimes into data file...");
            database_signals.lock().unwrap().store_file().unwrap();
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
    let auto_save_interval_seconds = cfg.auto_save_interval_seconds.clone();
    thread::spawn(move || loop {
        thread::sleep(std::time::Duration::from_secs(auto_save_interval_seconds));
        println!("Auto-Save");
        let _lock = file_mutex_auto_save.lock();
        database_autosave.lock().unwrap().store_file().unwrap();
    });

    // monitor terminal input:
    // TODO
    let overtime_end: chrono::DateTime<chrono::offset::Local> = std::time::SystemTime::now().into();
    let overtime = database.lock().unwrap().calculate_overtime(
        chrono::Duration::hours(cfg.weekly_hours),
        (cfg.cutoff_datetime, overtime_end),
    ) + chrono::Duration::seconds((cfg.cutoff_day_overtime_hours * 3600.0) as i64);
    println!("overtime: {}", format_chrono_duration(&overtime));

    loop {
        thread::sleep(std::time::Duration::from_secs(2));
        activity_recorder
            .lock()
            .unwrap()
            .handle_event(EventType::Commit);
        println!("---");
        database.lock().unwrap().print_vertical_timeline();
    }
}