//! Activity recording and session management
//!
//! This module handles storing work sessions reported by platform-specific detectors.

use std::sync::*;
use crate::models::WorktimeEntry;
use crate::database::Database;
use crate::idle_popup::IdleGapInfo;
use super::WorkSession;

/// Sender type for idle-gap events.
/// When `ask_on_idle` is enabled the recorder sends gap info to the main thread
/// which shows the popup and (on confirmation) inserts the gap into the database.
pub type IdleGapSender = std::sync::mpsc::Sender<IdleGapInfo>;

/// Records work sessions from platform-specific idle detectors
///
/// The recorder receives completed work sessions from detectors and stores them
/// in the database. It also manages comments for sessions.
pub struct ActivityRecorder {
    database: Arc<Mutex<Database>>,
    comments: String,
    /// Optional channel to send idle-gap info to the popup handler.
    /// `None` when `ask_on_idle = false`.
    idle_gap_tx: Option<IdleGapSender>,
    /// When a session ends, we record its end time here as the start of a
    /// potential idle gap.  The gap is only sent once the *next* session
    /// starts so we know the full gap duration (end = next session start).
    pending_gap_start: Option<chrono::DateTime<chrono::Local>>,
}

impl ActivityRecorder {
    /// Create a new activity recorder
    ///
    /// # Arguments
    /// * `database` - Shared database for storing work sessions
    /// * `idle_gap_tx` - Optional sender for idle-gap popup events
    pub fn new(
        database: Arc<Mutex<Database>>,
        idle_gap_tx: Option<IdleGapSender>,
    ) -> Self {
        ActivityRecorder {
            database,
            comments: String::new(),
            idle_gap_tx,
            pending_gap_start: None,
        }
    }

    /// Handle a completed work session from the idle detector
    ///
    /// # Arguments
    /// * `session` - Completed work session to store
    pub fn handle_session(&mut self, session: WorkSession) {
        eprintln!("[popup-debug] handle_session called: start={} end={}",
            session.start.format("%H:%M:%S"), session.end.format("%H:%M:%S"));

        self.database
            .lock()
            .unwrap()
            .commit_worktime(WorktimeEntry {
                start: session.start,
                end: session.end,
                comments: self.comments.clone(),
            });

        // Phase 2 — record this session's end as the start of the next gap.
        self.pending_gap_start = Some(session.end);

        // Clear comments after committing
        self.comments.clear();
    }

    /// Called when a new session starts (user returned from idle).
    /// If there is a pending gap, its end time is now known — send it.
    pub fn notify_session_start(&mut self, session_start: chrono::DateTime<chrono::Local>) {
        if let (Some(tx), Some(gap_start)) = (self.idle_gap_tx.as_ref(), self.pending_gap_start) {
            let gap_end = session_start;
            eprintln!("[popup-debug] user returned: gap {} → {} ({} seconds)",
                gap_start.format("%H:%M:%S"), gap_end.format("%H:%M:%S"),
                (gap_end - gap_start).num_seconds());
            if gap_end > gap_start {
                match tx.send(IdleGapInfo { gap_start, gap_end }) {
                    Ok(_)  => eprintln!("[popup-debug] gap sent to channel OK"),
                    Err(e) => eprintln!("[popup-debug] FAILED to send gap to channel: {}", e),
                }
            } else {
                eprintln!("[popup-debug] gap skipped: zero or negative duration");
            }
            self.pending_gap_start = None;
        }
    }

    /// Add a comment to the next work session
    ///
    /// # Arguments
    /// * `comment` - Comment text to append
    pub fn add_comment(&mut self, comment: &str) {
        self.comments.push_str(comment);
    }

    /// Manually commit a work session
    ///
    /// This is typically called on application shutdown (e.g., SIGINT)
    /// to ensure the current session is saved.
    ///
    /// # Arguments
    /// * `session` - Work session to commit
    pub fn commit_session(&mut self, session: WorkSession) {
        self.database
            .lock()
            .unwrap()
            .commit_worktime(WorktimeEntry {
                start: session.start,
                end: session.end,
                comments: self.comments.clone(),
            });
        // NOTE: do NOT touch pending_gap_start here.
        // commit_session is called for in-progress snapshots (auto-save) and
        // on shutdown — neither is a real session boundary, so the pending gap
        // must survive until the user actually returns and handle_session fires.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handle_session() {
        // Create temporary database
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join("test_worktime_recorder.csv");
        let special_path = temp_dir.join("test_special_recorder.csv");

        let db = Arc::new(Mutex::new(
            Database::init(db_path.clone(), special_path.clone()).unwrap()
        ));

        let mut recorder = ActivityRecorder::new(db.clone(), None);

        // Create a work session
        let start = chrono::Local::now();
        let end = start + chrono::Duration::hours(2);
        let session = WorkSession { start, end };

        recorder.handle_session(session);

        // Should have committed one session
        let entries = db.lock().unwrap().rows.len();
        assert_eq!(entries, 1, "Should commit session");

        // Cleanup
        let _ = std::fs::remove_file(db_path);
        let _ = std::fs::remove_file(special_path);
    }

    #[test]
    fn test_gap_not_sent_when_no_tx() {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join("test_worktime_recorder2.csv");
        let special_path = temp_dir.join("test_special_recorder2.csv");

        let db = Arc::new(Mutex::new(
            Database::init(db_path.clone(), special_path.clone()).unwrap()
        ));

        // No sender → gaps silently ignored
        let mut recorder = ActivityRecorder::new(db.clone(), None);

        let t1 = chrono::Local::now();
        recorder.handle_session(WorkSession { start: t1, end: t1 + chrono::Duration::hours(1) });
        let t2 = t1 + chrono::Duration::hours(2);
        recorder.handle_session(WorkSession { start: t2, end: t2 + chrono::Duration::hours(1) });

        assert_eq!(db.lock().unwrap().rows.len(), 2);

        let _ = std::fs::remove_file(db_path);
        let _ = std::fs::remove_file(special_path);
    }

    #[test]
    fn test_gap_sent_on_return() {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join("test_worktime_recorder3.csv");
        let special_path = temp_dir.join("test_special_recorder3.csv");

        let db = Arc::new(Mutex::new(
            Database::init(db_path.clone(), special_path.clone()).unwrap()
        ));

        let (tx, rx) = std::sync::mpsc::channel();
        let mut recorder = ActivityRecorder::new(db.clone(), Some(tx));

        let t1 = chrono::Local::now();
        let session1_end = t1 + chrono::Duration::hours(1);
        // Session 1 ends — records pending_gap_start, no gap sent yet
        recorder.handle_session(WorkSession { start: t1, end: session1_end });
        assert!(rx.try_recv().is_err(), "No gap sent until user returns");

        // User returns after a 10-minute gap — notify_session_start fires the gap
        let session2_start = session1_end + chrono::Duration::minutes(10);
        recorder.notify_session_start(session2_start);

        let gap = rx.try_recv().expect("Gap should be sent when user returns");
        assert_eq!(gap.gap_start, session1_end);
        assert_eq!(gap.gap_end, session2_start);

        let _ = std::fs::remove_file(db_path);
        let _ = std::fs::remove_file(special_path);
    }

}
