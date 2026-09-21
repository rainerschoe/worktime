//! Background monitoring: idle detection, auto-save, SIGINT handler.
//!
//! This module runs entirely on a background thread.  It communicates with the
//! rest of the application only through:
//!   - `DbHandle`  — a cheap-to-clone handle whose mutex is locked only briefly
//!   - `mpsc` channels — for sending idle-gap events to the UI thread

use std::thread;
use signal_hook::{consts::SIGINT, iterator::Signals};
use crate::config::Config;
use crate::database::DbHandle;
use crate::idle_detection::{ActivityRecorder, WorkSession, create_idle_detector};
use crate::idle_popup::IdleGapInfo;
use crate::utils::format_chrono_duration;

/// Start idle detection and auto-save on the calling (background) thread.
///
/// Returns the channel receiver the UI thread should poll for idle-gap events.
/// Blocks forever — call from a `thread::spawn`.
pub fn run_monitoring(
    db: DbHandle,
    cfg: &Config,
) -> Option<std::sync::mpsc::Receiver<IdleGapInfo>> {
    // ── Idle-gap channel (UI thread receives gap events) ─────────────────────
    let (idle_gap_tx, gap_rx_opt) = if cfg.ask_on_idle {
        let (tx, rx) = std::sync::mpsc::channel::<IdleGapInfo>();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    // ── Activity recorder ────────────────────────────────────────────────────
    // Owns its own internal state; takes DbHandle (lock held only per-call).
    let recorder = ActivityRecorder::new(db.clone(), idle_gap_tx);

    // ── Idle detector ────────────────────────────────────────────────────────
    let mut detector = create_idle_detector(cfg.timeout_minutes)
        .unwrap_or_else(|e| {
            eprintln!("Failed to create idle detector:\n{}", e);
            std::process::exit(1);
        });

    // The detector fires callbacks on its own internal thread.
    // We wrap the recorder in an Arc<Mutex> only here, so the callbacks can
    // call it — the lock is held only for the duration of one short method.
    let recorder = std::sync::Arc::new(std::sync::Mutex::new(recorder));
    let rec_end   = recorder.clone();
    let rec_start = recorder.clone();

    detector.start_monitoring(
        Box::new(move |session: WorkSession| {
            rec_end.lock().unwrap().handle_session(session);
        }),
        Box::new(move |start| {
            rec_start.lock().unwrap().notify_session_start(start);
        }),
    ).expect("Failed to start idle monitoring");

    // ── SIGINT handler ───────────────────────────────────────────────────────
    let db_sig  = db.clone();
    let det_sig = std::sync::Arc::new(std::sync::Mutex::new(detector));
    let rec_sig = recorder.clone();
    let det_sig2 = det_sig.clone();
    thread::spawn(move || {
        let mut signals = Signals::new(&[SIGINT]).unwrap();
        for _sig in signals.forever() {
            let session = det_sig2.lock().unwrap().get_current_session();
            if let Some(s) = session {
                rec_sig.lock().unwrap().commit_session(s);
            }
            let _ = db_sig.store_file();
            std::process::exit(0);
        }
    });

    // ── Auto-save loop ───────────────────────────────────────────────────────
    let db_save  = db.clone();
    let det_save = det_sig.clone();
    let rec_save = recorder.clone();
    let interval = cfg.auto_save_interval_seconds;
    thread::spawn(move || loop {
        thread::sleep(std::time::Duration::from_secs(interval));
        let session = det_save.lock().unwrap().get_current_session();
        if let Some(s) = session {
            rec_save.lock().unwrap().commit_session(s);
        }
        let _ = db_save.store_file();
        eprintln!("Auto-Save");
    });

    // ── Terminal status loop ─────────────────────────────────────────────────
    let overtime_end: chrono::DateTime<chrono::offset::Local> = std::time::SystemTime::now().into();
    let overtime = db.with(|d| {
        d.calculate_overtime(
            chrono::Duration::hours(cfg.weekly_hours),
            (cfg.cutoff_datetime, overtime_end),
        )
    }) + chrono::Duration::seconds((cfg.cutoff_day_overtime_hours * 3600.0) as i64);
    println!("overtime: {}", format_chrono_duration(&overtime));

    gap_rx_opt
}
