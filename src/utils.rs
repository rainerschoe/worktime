pub fn format_chrono_duration(duration: &chrono::Duration) -> String {
    let sec_total = duration.num_seconds();
    let hours = sec_total / 60 / 60;
    let mins = (sec_total - hours * 60 * 60) / 60;
    let secs = sec_total - hours * 60 * 60 - mins * 60;
    format!("{}h{}m{}s", hours, mins, secs)
}
