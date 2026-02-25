use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Config {
    pub timeout_minutes: u64,
    pub data_file: String,
    pub special_day_file: String,
    pub auto_save_interval_seconds: u64,
    pub weekly_hours: i64,
    pub cutoff_day_overtime_hours: f64,
    pub cutoff_datetime: chrono::DateTime<chrono::offset::Local>,
}

impl ::std::default::Default for Config {
    fn default() -> Self {
        Self {
            timeout_minutes: 10,
            data_file: "~/.worktime.csv".into(),
            special_day_file: "~/.special_days.csv".into(),
            auto_save_interval_seconds: 30,
            weekly_hours: 30,
            cutoff_day_overtime_hours: 0.0,
            cutoff_datetime: "2023-05-01T00:00:00.00+02:00".parse().unwrap(),
        }
    }
}