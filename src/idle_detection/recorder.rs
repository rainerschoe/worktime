//! Activity recording and session management
//! 
//! This module handles storing work sessions reported by platform-specific detectors.

use std::sync::*;
use crate::models::WorktimeEntry;
use crate::database::Database;
use super::WorkSession;

/// Records work sessions from platform-specific idle detectors
/// 
/// The recorder receives completed work sessions from detectors and stores them
/// in the database. It also manages comments for sessions.
pub struct ActivityRecorder {
    database: Arc<Mutex<Database>>,
    comments: String,
}

impl ActivityRecorder {
    /// Create a new activity recorder
    /// 
    /// # Arguments
    /// * `database` - Shared database for storing work sessions
    pub fn new(database: Arc<Mutex<Database>>) -> Self {
        ActivityRecorder {
            database,
            comments: String::new(),
        }
    }

    /// Handle a completed work session from the idle detector
    /// 
    /// # Arguments
    /// * `session` - Completed work session to store
    pub fn handle_session(&mut self, session: WorkSession) {
        self.database
            .lock()
            .unwrap()
            .commit_worktime(WorktimeEntry {
                start: session.start,
                end: session.end,
                comments: self.comments.clone(),
            });
        
        // Clear comments after committing
        self.comments.clear();
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
        
        let mut recorder = ActivityRecorder::new(db.clone());
        
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
}