//! Activity recording and session management.
//!
//! Receives completed work sessions from the idle detector and commits them to
//! the database via `DbHandle` (lock held only for the duration of each call).

use crate::models::WorktimeEntry;
use crate::database::DbHandle;
use crate::idle_popup::IdleGapInfo;
use super::WorkSession;

pub struct ActivityRecorder {
    db: DbHandle,
    /// Optional sender for idle-gap events → UI thread popup.
    idle_gap_tx: Option<std::sync::mpsc::Sender<IdleGapInfo>>,
    /// End time of the last completed session; becomes gap_start when user returns.
    pending_gap_start: Option<chrono::DateTime<chrono::Local>>,
}

impl ActivityRecorder {
    pub fn new(
        db: DbHandle,
        idle_gap_tx: Option<std::sync::mpsc::Sender<IdleGapInfo>>,
    ) -> Self {
        ActivityRecorder { db, idle_gap_tx, pending_gap_start: None }
    }

    /// Called when a work session ends (user went idle).
    pub fn handle_session(&mut self, session: WorkSession) {
        eprintln!("[recorder] session end: {} → {}",
            session.start.format("%H:%M:%S"), session.end.format("%H:%M:%S"));
        self.db.commit_worktime(WorktimeEntry {
            start: session.start,
            end:   session.end,
            comments: String::new(),
        });
        self.pending_gap_start = Some(session.end);
    }

    /// Called when the user returns from idle (new session starts).
    /// Fires the idle-gap event if a gap is pending.
    pub fn notify_session_start(&mut self, session_start: chrono::DateTime<chrono::Local>) {
        if let (Some(tx), Some(gap_start)) = (self.idle_gap_tx.as_ref(), self.pending_gap_start) {
            let gap_end = session_start;
            if gap_end > gap_start {
                eprintln!("[recorder] gap {} → {}", gap_start.format("%H:%M:%S"), gap_end.format("%H:%M:%S"));
                let _ = tx.send(IdleGapInfo { gap_start, gap_end });
            }
            self.pending_gap_start = None;
        }
    }

    /// Snapshot-commit for auto-save / shutdown (does not affect pending gap).
    pub fn commit_session(&mut self, session: WorkSession) {
        self.db.commit_worktime(WorktimeEntry {
            start: session.start,
            end:   session.end,
            comments: String::new(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::DbHandle;

    fn make_db() -> DbHandle {
        let temp_dir = std::env::temp_dir();
        let db_path  = temp_dir.join(format!("test_recorder_{}.csv", std::process::id()));
        let sp_path  = temp_dir.join(format!("test_recorder_sp_{}.csv", std::process::id()));
        DbHandle::open(db_path, sp_path).unwrap()
    }

    #[test]
    fn test_handle_session() {
        let db = make_db();
        let mut r = ActivityRecorder::new(db.clone(), None);
        let start = chrono::Local::now();
        r.handle_session(WorkSession { start, end: start + chrono::Duration::hours(2) });
        assert_eq!(db.with(|d| d.rows.len()), 1);
    }

    #[test]
    fn test_gap_sent_on_return() {
        let db = make_db();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut r = ActivityRecorder::new(db, Some(tx));
        let t1 = chrono::Local::now();
        let end1 = t1 + chrono::Duration::hours(1);
        r.handle_session(WorkSession { start: t1, end: end1 });
        assert!(rx.try_recv().is_err(), "no gap yet");
        let t2 = end1 + chrono::Duration::minutes(10);
        r.notify_session_start(t2);
        let gap = rx.try_recv().expect("gap sent");
        assert_eq!(gap.gap_start, end1);
        assert_eq!(gap.gap_end, t2);
    }
}
