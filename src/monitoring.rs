use std::sync::*;
use std::thread;
use signal_hook::{consts::SIGINT, iterator::Signals};
use crate::config::Config;
use crate::database::Database;
use crate::idle_detection::{ActivityRecorder, create_idle_detector};
use crate::idle_popup::IdleGapInfo;
use crate::utils::format_chrono_duration;

/// Set up idle detection, auto-save, and SIGINT handler, then hand the
/// idle-gap `Receiver` back to the caller via `rx_out` before entering the
/// blocking terminal-status loop.
///
/// Call this on a **background thread**.  The function blocks forever (until
/// SIGINT / process exit).  The `rx_out` sender fires exactly once, before the
/// blocking loop starts, so the caller can retrieve the receiver without any
/// race condition.
pub fn run_interactive_monitoring(
    database: Arc<Mutex<Database>>,
    cfg: &Config,
    rx_out: std::sync::mpsc::SyncSender<Option<std::sync::mpsc::Receiver<IdleGapInfo>>>,
) {
    // ── Idle-gap popup channel ───────────────────────────────────────────────
    // When ask_on_idle is enabled, the recorder sends IdleGapInfo over this
    // channel.  The receiver is passed back to the UI thread via rx_out.
    let (idle_gap_tx_for_recorder, idle_gap_rx): (
        Option<std::sync::mpsc::Sender<IdleGapInfo>>,
        Option<std::sync::mpsc::Receiver<IdleGapInfo>>,
    ) = if cfg.ask_on_idle {
        let (tx, rx) = std::sync::mpsc::channel::<IdleGapInfo>();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    // ── Activity recorder ────────────────────────────────────────────────────
    let activity_recorder = Arc::new(Mutex::new(ActivityRecorder::new(
        database.clone(),
        idle_gap_tx_for_recorder,
    )));

    // Mutex used to synchronise auto-save with SIGINT handling.
    let file_mutex_signal    = Arc::new(Mutex::new(()));
    let file_mutex_auto_save = file_mutex_signal.clone();

    // ── Idle detector ────────────────────────────────────────────────────────
    let mut idle_detector = create_idle_detector(cfg.timeout_minutes)
        .unwrap_or_else(|e| {
            eprintln!("Failed to create idle detector:\n{}", e);
            std::process::exit(1);
        });

    let activity_recorder_on_end   = activity_recorder.clone();
    let activity_recorder_on_start = activity_recorder.clone();
    idle_detector
        .start_monitoring(
            Box::new(move |session| {
                activity_recorder_on_end
                    .lock()
                    .unwrap()
                    .handle_session(session);
            }),
            Box::new(move |session_start| {
                activity_recorder_on_start
                    .lock()
                    .unwrap()
                    .notify_session_start(session_start);
            }),
        )
        .expect("Failed to start idle monitoring");

    let idle_detector_ref = Arc::new(Mutex::new(idle_detector));

    // ── SIGINT handler ───────────────────────────────────────────────────────
    let database_signals          = database.clone();
    let idle_detector_signals     = idle_detector_ref.clone();
    let activity_recorder_signals = activity_recorder.clone();
    thread::spawn(move || {
        let mut signals = Signals::new(&[SIGINT]).unwrap();
        for sig in signals.forever() {
            println!("Received signal {:?}", sig);

            let detector = idle_detector_signals.lock().unwrap();
            if let Some(session) = detector.get_current_session() {
                activity_recorder_signals
                    .lock()
                    .unwrap()
                    .commit_session(session);
            }

            println!("Saving worktimes into data file...");
            database_signals.lock().unwrap().store_file().unwrap();
            let _lock = file_mutex_signal.lock();
            std::process::exit(0);
        }
    });

    // ── Auto-save ────────────────────────────────────────────────────────────
    let database_autosave          = database.clone();
    let idle_detector_autosave     = idle_detector_ref.clone();
    let activity_recorder_autosave = activity_recorder.clone();
    let auto_save_interval_seconds = cfg.auto_save_interval_seconds;
    thread::spawn(move || loop {
        thread::sleep(std::time::Duration::from_secs(auto_save_interval_seconds));
        let _lock = file_mutex_auto_save.lock();
        if let Some(session) = idle_detector_autosave.lock().unwrap().get_current_session() {
            activity_recorder_autosave.lock().unwrap().commit_session(session);
        }
        println!("Auto-Save");
        database_autosave.lock().unwrap().store_file().unwrap();
    });

    // ── Hand the gap receiver back to the UI thread ──────────────────────────
    // This fires before the blocking loop below, so the UI thread never waits.
    let _ = rx_out.send(idle_gap_rx);

    // ── Live terminal status loop (blocks forever) ───────────────────────────
    let overtime_end: chrono::DateTime<chrono::offset::Local> =
        std::time::SystemTime::now().into();
    let overtime = database.lock().unwrap().calculate_overtime(
        chrono::Duration::hours(cfg.weekly_hours),
        (cfg.cutoff_datetime, overtime_end),
    ) + chrono::Duration::seconds((cfg.cutoff_day_overtime_hours * 3600.0) as i64);
    println!("overtime: {}", format_chrono_duration(&overtime));

    loop {
        thread::sleep(std::time::Duration::from_secs(2));
        println!("---");

        let detector = idle_detector_ref.lock().unwrap();
        let current_session = detector.get_current_session();
        let current_start   = current_session.map(|s| s.start);
        let idle_duration   = detector.get_idle_duration();
        drop(detector);

        database.lock().unwrap().print_vertical_timeline_with_current(current_start);

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
