use std::sync::*;
use rdev::Event;
use crate::models::WorktimeEntry;
use crate::database::Database;

pub enum EventType {
    Activity(Event),
    _Comment(String), // TODO: implement comment user interface
    Commit,
}

pub struct ActivityRecorder {
    database: Arc<Mutex<Database>>,
    last_event_time: chrono::DateTime<chrono::offset::Local>,
    last_start_time: chrono::DateTime<chrono::offset::Local>,
    comments: String,
    timeouts_minutes: u64,
}

impl ActivityRecorder {
    pub fn new(database: Arc<Mutex<Database>>, timeout_minutes: u64) -> Self {
        let now = std::time::SystemTime::now().into();
        ActivityRecorder {
            database: database,
            last_event_time: now,
            last_start_time: now,
            comments: "".into(),
            timeouts_minutes: timeout_minutes,
        }
    }

    pub fn handle_event(self: &mut Self, event: EventType) {
        match event {
            EventType::Activity(event) => {
                let event_time: chrono::DateTime<chrono::offset::Local> = event.time.into();
                let time_since_last_activity = event_time - self.last_event_time;
                if time_since_last_activity
                    > chrono::Duration::minutes(self.timeouts_minutes as i64)
                {
                    self.database
                        .lock()
                        .unwrap()
                        .commit_worktime(WorktimeEntry {
                            start: self.last_start_time,
                            end: self.last_event_time,
                            comments: self.comments.clone(),
                        });
                    self.last_start_time = event_time;
                    self.comments.clear();
                }
                self.last_event_time = event_time;
            }
            EventType::_Comment(comment) => {
                self.comments.push_str(&comment);
            }
            EventType::Commit => {
                self.database
                    .lock()
                    .unwrap()
                    .commit_worktime(WorktimeEntry {
                        start: self.last_start_time,
                        end: self.last_event_time,
                        comments: self.comments.clone(),
                    });
            }
        }
    }
}
