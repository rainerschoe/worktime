use std::sync::*;
use std::thread;
use signal_hook::{consts::SIGINT, iterator::Signals};
use crate::config::Config;
use crate::database::Database;
use crate::idle_detection::{ActivityRecorder, create_idle_detector};
use crate::utils::format_chrono_duration;

pub fn run_interactive_monitoring(database: Arc<Mutex<Database>>, cfg: &Config) {
    let activity_recorder = Arc::new(Mutex::new(ActivityRecorder::new(
        database.clone(),
    )));

    // Mutex used to syncronize auto-save with sigint handling (in order to avoid terminating in the middle of writing into file)
    // Note: termination is a bit messy right now, as I do not cleanly join all threads, so I need hacks like this
    let file_mutex_signal = Arc::new(Mutex::new(()));
    let file_mutex_auto_save = file_mutex_signal.clone();

    // Create platform-specific idle detector and start monitoring
    let mut idle_detector = create_idle_detector(cfg.timeout_minutes)
        .expect("Failed to create idle detector");
    
    // Start idle detection monitoring
    let activity_recorder_monitor = activity_recorder.clone();
    idle_detector
        .start_monitoring(Box::new(move |session| {
            activity_recorder_monitor
                .lock()
                .unwrap()
                .handle_session(session);
        }))
        .expect("Failed to start idle monitoring");
    
    // Store detector for signal handling
    let idle_detector_ref = Arc::new(Mutex::new(idle_detector));

    // monitor signals:
    let database_signals = database.clone();
    let idle_detector_signals = idle_detector_ref.clone();
    let activity_recorder_signals = activity_recorder.clone();
    thread::spawn(move || {
        let mut signals = Signals::new(&[SIGINT]).unwrap();
        for sig in signals.forever() {
            println!("Received signal {:?}", sig);
            
            // Get current session and commit it
            let detector = idle_detector_signals.lock().unwrap();
            if let Some(session) = detector.get_current_session() {
                activity_recorder_signals
                    .lock()
                    .unwrap()
                    .commit_session(session);
            }
            
            println!("Saving worktimes into data file...");
            database_signals.lock().unwrap().store_file().unwrap();
            // lock mutex here, which prevents any auto-save to try saving while we exit
            let _lock = file_mutex_signal.lock();
            std::process::exit(0);
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
        println!("---");
        
        // Get current in-progress session from the idle detector to show live duration
        let detector = idle_detector_ref.lock().unwrap();
        let current_session = detector.get_current_session();
        let current_start = current_session.map(|s| s.start);
        
        // Get idle duration for live activity status
        let idle_duration = detector.get_idle_duration();
        drop(detector); // Release lock before printing
        
        database.lock().unwrap().print_vertical_timeline_with_current(current_start);
        
        // Display live activity state AFTER the timeline
        if let Some(idle_dur) = idle_duration {
            let idle_secs = idle_dur.num_seconds();
            if idle_secs < 60 {
                println!("Active ({}s since last input)", idle_secs);
            } else {
                let idle_mins = idle_secs / 60;
                println!("Idle for {}m {}s", idle_mins, idle_secs % 60);
            }
        } else {
            println!("No activity detected yet");
        }
    }
}