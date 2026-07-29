//! Idle detection using rdev for Windows and macOS
//! 
//! This module uses the rdev library to monitor keyboard and mouse events
//! and determines when the user has been idle for longer than the timeout.

use std::sync::{Arc, Mutex};
use std::thread;
use rdev::{listen, Event};
use super::{IdleDetector, WorkSession, SessionCallback, SessionStartCallback};

/// Idle detector using rdev for input event monitoring
pub struct RdevIdleDetector {
    timeout_minutes: u64,
    session_start: Arc<Mutex<Option<chrono::DateTime<chrono::Local>>>>,
    last_activity: Arc<Mutex<Option<chrono::DateTime<chrono::Local>>>>,
}

impl RdevIdleDetector {
    /// Create a new rdev-based idle detector
    /// 
    /// # Arguments
    /// * `timeout_minutes` - Idle timeout in minutes
    pub fn new(timeout_minutes: u64) -> Self {
        RdevIdleDetector {
            timeout_minutes,
            session_start: Arc::new(Mutex::new(None)),
            last_activity: Arc::new(Mutex::new(None)),
        }
    }
}

impl IdleDetector for RdevIdleDetector {
    fn start_monitoring(
        &mut self,
        on_session_end: SessionCallback,
        on_session_start: SessionStartCallback,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let timeout_minutes = self.timeout_minutes;
        let on_session_end   = Arc::new(Mutex::new(on_session_end));
        let on_session_start = Arc::new(Mutex::new(on_session_start));
        let session_start = self.session_start.clone();
        let last_activity = self.last_activity.clone();

        // Initialize with current time
        let now = chrono::Local::now();
        *session_start.lock().unwrap() = Some(now);
        *last_activity.lock().unwrap() = Some(now);
        on_session_start.lock().unwrap()(now);

        // Start listening to input events
        thread::spawn(move || {
            if let Err(error) = listen(move |event: Event| {
                let event_time: chrono::DateTime<chrono::Local> = event.time.into();

                let mut session_start_guard = session_start.lock().unwrap();
                let mut last_activity_guard = last_activity.lock().unwrap();

                if let (Some(start), Some(last)) = (*session_start_guard, *last_activity_guard) {
                    let idle_duration = event_time - last;
                    if idle_duration >= chrono::Duration::minutes(timeout_minutes as i64) {
                        // Session ended — commit it
                        on_session_end.lock().unwrap()(WorkSession { start, end: last });
                        // New session starting — notify
                        *session_start_guard = Some(event_time);
                        on_session_start.lock().unwrap()(event_time);
                    }
                }

                *last_activity_guard = Some(event_time);
            }) {
                eprintln!("Error in rdev event listener: {:?}", error);
            }
        });

        Ok(())
    }
    
    fn stop(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Thread will be terminated when the detector is dropped
        // rdev::listen runs in a loop and doesn't provide a clean stop mechanism
        Ok(())
    }
    
    fn get_current_session(&self) -> Option<WorkSession> {
        let session_start = self.session_start.lock().unwrap();
        let last_activity = self.last_activity.lock().unwrap();
        
        if let (Some(start), Some(end)) = (*session_start, *last_activity) {
            Some(WorkSession { start, end })
        } else {
            None
        }
    }
    
    fn get_idle_duration(&self) -> Option<chrono::Duration> {
        let last_activity = self.last_activity.lock().unwrap();
        last_activity.map(|last| chrono::Local::now() - last)
    }
}