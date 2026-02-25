#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq, PartialOrd, Eq, Ord, Clone)]
pub struct WorktimeEntry {
    pub start: chrono::DateTime<chrono::offset::Local>,
    pub end: chrono::DateTime<chrono::offset::Local>,
    //comments: Vec<String>,
    pub comments: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq, PartialOrd, Eq, Ord, Clone)]
pub enum SpecialDayType {
    Vacation,
    Sick,
    Leave,
    Holiday,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq, PartialOrd, Eq, Ord, Clone)]
pub struct SpecialDayEntry {
    pub day: chrono::naive::NaiveDate,
    pub day_type: SpecialDayType,
}

impl WorktimeEntry {
    pub fn duration(self: &Self) -> chrono::Duration {
        return self.end - self.start;
    }
}