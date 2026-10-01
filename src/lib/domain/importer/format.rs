//! Numbers and durations as the log prints them.

/// `1234567` as `1,234,567`.
pub fn format_number(n: usize) -> String {
    let digits = n.to_string();
    let mut result = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(ch);
    }
    result
}

/// Whole seconds as `1d 2h 3m 4s`, dropping zero parts, never empty.
pub fn format_duration(seconds: f64) -> String {
    let total_seconds = seconds as u64;
    let days = total_seconds / 86400;
    let hours = (total_seconds % 86400) / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let secs = total_seconds % 60;

    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    if secs > 0 || parts.is_empty() {
        parts.push(format!("{secs}s"));
    }
    parts.join(" ")
}

/// Time left for `remaining_rows` at `avg_time_per_row` seconds each, or
/// `unknown` before any row has been timed.
pub fn format_eta(remaining_rows: usize, avg_time_per_row: f64) -> String {
    if avg_time_per_row == 0.0 {
        return "unknown".to_string();
    }
    format_duration(remaining_rows as f64 * avg_time_per_row)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_group_thousands() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1000), "1,000");
        assert_eq!(format_number(1234567), "1,234,567");
    }

    #[test]
    fn durations_drop_zero_parts_and_never_go_empty() {
        assert_eq!(format_duration(0.0), "0s");
        assert_eq!(format_duration(0.9), "0s");
        assert_eq!(format_duration(61.0), "1m 1s");
        assert_eq!(format_duration(3600.0), "1h");
        assert_eq!(format_duration(90061.0), "1d 1h 1m 1s");
    }

    #[test]
    fn eta_is_unknown_until_a_row_has_been_timed() {
        assert_eq!(format_eta(500, 0.0), "unknown");
        assert_eq!(format_eta(500, 0.5), "4m 10s");
    }
}
