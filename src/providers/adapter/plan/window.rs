//! How long a plan window lasts, said in words.

pub(super) fn window_label_from_minutes(minutes: f64) -> String {
    let minutes = minutes.max(0.0).round() as i64;
    let hour = 60;
    let day = 24 * hour;
    if minutes >= day && minutes % day == 0 {
        let days = minutes / day;
        return format!("{days} day{}", if days == 1 { "" } else { "s" });
    }
    if minutes >= hour && minutes % hour == 0 {
        let hours = minutes / hour;
        return format!("{hours} hour{}", if hours == 1 { "" } else { "s" });
    }
    format!("{minutes} minutes")
}
