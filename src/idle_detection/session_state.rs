//! Session state machine for idle detection
//!
//! This module provides a testable state machine for managing work sessions
//! independently of input device monitoring.

use chrono::{DateTime, Duration, Local};
use super::WorkSession;

/// State of the session tracking
#[derive(Debug, Clone, PartialEq)]
enum SessionState {
    /// No active session, waiting for activity to start one
    Idle,
    /// Active session in progress
    Active {
        start: DateTime<Local>,
        last_activity: DateTime<Local>,
    },
}

/// Session state machine that tracks work sessions based on activity events
pub struct SessionStateMachine {
    state: SessionState,
    timeout_minutes: u64,
}

impl SessionStateMachine {
    /// Create a new session state machine
    ///
    /// # Arguments
    /// * `timeout_minutes` - Idle timeout in minutes
    pub fn new(timeout_minutes: u64) -> Self {
        SessionStateMachine {
            state: SessionState::Idle,
            timeout_minutes,
        }
    }

    /// Process an activity event at the given time
    ///
    /// # Arguments
    /// * `now` - Current timestamp when activity occurred
    ///
    /// # Returns
    /// * `Some(WorkSession)` - If a session was completed due to timeout
    /// * `None` - If no session was completed
    pub fn on_activity(&mut self, now: DateTime<Local>) -> Option<WorkSession> {
        match self.state {
            SessionState::Idle => {
                // Start a new session
                self.state = SessionState::Active {
                    start: now,
                    last_activity: now,
                };
                None
            }
            SessionState::Active { start, .. } => {
                // Activity detected - just update last activity time
                // Don't check timeout here - that's done in check_timeout()
                self.state = SessionState::Active {
                    start,
                    last_activity: now,
                };
                None
            }
        }
    }

    /// Check if a session should be completed based on current time (no activity)
    ///
    /// # Arguments
    /// * `now` - Current timestamp
    ///
    /// # Returns
    /// * `Some(WorkSession)` - If a session should be completed due to timeout
    /// * `None` - If no session should be completed
    pub fn check_timeout(&mut self, now: DateTime<Local>) -> Option<WorkSession> {
        match self.state {
            SessionState::Idle => None,
            SessionState::Active { start, last_activity } => {
                let idle_duration = now - last_activity;
                
                if idle_duration >= Duration::minutes(self.timeout_minutes as i64) {
                    // Session ended due to timeout
                    let completed_session = WorkSession {
                        start,
                        end: last_activity,
                    };
                    
                    self.state = SessionState::Idle;
                    Some(completed_session)
                } else {
                    None
                }
            }
        }
    }

    /// Get the current session if one is active
    pub fn get_current_session(&self) -> Option<WorkSession> {
        match self.state {
            SessionState::Idle => None,
            SessionState::Active { start, last_activity } => Some(WorkSession {
                start,
                end: last_activity,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_initial_activity_starts_session() {
        let mut sm = SessionStateMachine::new(5);
        let t1 = Local.with_ymd_and_hms(2024, 1, 1, 9, 0, 0).unwrap();
        
        let result = sm.on_activity(t1);
        assert_eq!(result, None, "First activity should not complete a session");
        
        let current = sm.get_current_session();
        assert!(current.is_some(), "Should have an active session");
        assert_eq!(current.unwrap().start, t1);
    }

    #[test]
    fn test_activity_within_timeout_continues_session() {
        let mut sm = SessionStateMachine::new(5);
        let t1 = Local.with_ymd_and_hms(2024, 1, 1, 9, 0, 0).unwrap();
        let t2 = t1 + Duration::minutes(2);
        let t3 = t2 + Duration::minutes(2);
        
        sm.on_activity(t1);
        let result2 = sm.on_activity(t2);
        assert_eq!(result2, None, "Activity within timeout should not complete session");
        
        let result3 = sm.on_activity(t3);
        assert_eq!(result3, None, "Activity within timeout should not complete session");
        
        let current = sm.get_current_session();
        assert!(current.is_some());
        let session = current.unwrap();
        assert_eq!(session.start, t1, "Session should still have original start time");
        assert_eq!(session.end, t3, "Session end should be updated to last activity");
    }

    #[test]
    fn test_activity_after_timeout_continues_session() {
        let mut sm = SessionStateMachine::new(5);
        let t1 = Local.with_ymd_and_hms(2024, 1, 1, 9, 0, 0).unwrap();
        let t2 = t1 + Duration::minutes(3); // Activity at 9:03
        let t3 = t2 + Duration::minutes(6); // Activity at 9:09 (6 minutes after last)
        
        // Start session
        sm.on_activity(t1);
        
        // Continue session
        sm.on_activity(t2);
        
        // Activity after timeout should NOT complete the session automatically
        // It just updates the last activity time
        let result = sm.on_activity(t3);
        assert_eq!(result, None, "Activity should not complete session - only check_timeout does that");
        
        // Should still have the same session, now with updated last activity
        let current = sm.get_current_session();
        assert!(current.is_some(), "Should still have active session");
        let session = current.unwrap();
        assert_eq!(session.start, t1, "Session should still start at t1");
        assert_eq!(session.end, t3, "Session end should be updated to t3");
    }

    #[test]
    fn test_check_timeout_without_activity() {
        let mut sm = SessionStateMachine::new(5);
        let t1 = Local.with_ymd_and_hms(2024, 1, 1, 9, 0, 0).unwrap();
        let t2 = t1 + Duration::minutes(3);
        let t3 = t2 + Duration::minutes(6); // 6 minutes after last activity
        
        // Start session
        sm.on_activity(t1);
        sm.on_activity(t2);
        
        // Check timeout without new activity
        let result = sm.check_timeout(t3);
        assert!(result.is_some(), "Should complete session after timeout");
        
        let completed = result.unwrap();
        assert_eq!(completed.start, t1);
        assert_eq!(completed.end, t2);
        
        // Should be idle now
        assert_eq!(sm.get_current_session(), None, "Should be idle after timeout");
    }

    #[test]
    fn test_multiple_sessions_with_breaks() {
        let mut sm = SessionStateMachine::new(5);
        
        // First session: 9:00 - 9:10
        let s1_start = Local.with_ymd_and_hms(2024, 1, 1, 9, 0, 0).unwrap();
        let s1_activity = s1_start + Duration::minutes(10);
        sm.on_activity(s1_start);
        sm.on_activity(s1_activity);
        
        // Check timeout at 9:16 (6 minutes after last activity)
        let timeout_check = s1_activity + Duration::minutes(6);
        let result1 = sm.check_timeout(timeout_check);
        assert!(result1.is_some());
        let session1 = result1.unwrap();
        assert_eq!(session1.start, s1_start);
        assert_eq!(session1.end, s1_activity);
        
        // Second session: 13:00 - 13:30
        let s2_start = Local.with_ymd_and_hms(2024, 1, 1, 13, 0, 0).unwrap();
        let s2_activity = s2_start + Duration::minutes(30);
        
        let result2 = sm.on_activity(s2_start);
        assert_eq!(result2, None, "Starting new session after idle should not complete anything");
        
        sm.on_activity(s2_activity);
        
        let current = sm.get_current_session();
        assert!(current.is_some());
        let session2 = current.unwrap();
        assert_eq!(session2.start, s2_start);
        assert_eq!(session2.end, s2_activity);
    }
}