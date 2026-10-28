//! Numbers and durations as the log prints them.

use std::time::Duration;

/// `1234567` as `1,234,567`.
pub fn format_number(n: usize) -> String {
    let digits = n.to_string();
    let mut result = String::with_capacity(digits.len().saturating_add(digits.len() / 3));
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && digits.len().saturating_sub(i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(ch);
    }
    result
}

const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 60 * SECONDS_PER_MINUTE;
const SECONDS_PER_DAY: u64 = 24 * SECONDS_PER_HOUR;

/// Whole seconds as `1d 2h 3m 4s`, dropping zero parts, never empty.
pub fn format_duration(seconds: f64) -> String {
    // Negative, NaN and out of range all read as zero.
    let total_seconds = Duration::try_from_secs_f64(seconds).map_or(0, |d| d.as_secs());
    let days = total_seconds / SECONDS_PER_DAY;
    let hours = (total_seconds % SECONDS_PER_DAY) / SECONDS_PER_HOUR;
    let minutes = (total_seconds % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE;
    let secs = total_seconds % SECONDS_PER_MINUTE;

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
    format_duration(count_to_f64(remaining_rows) * avg_time_per_row)
}

/// A count as an `f64`, for rates and averages. Counts past `u32::MAX` read
/// as `u32::MAX`, which no import comes near.
pub fn count_to_f64(count: usize) -> f64 {
    u32::try_from(count).map_or(f64::from(u32::MAX), f64::from)
}

/// The mean of `values`, or 0 when there are none.
pub fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / count_to_f64(values.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_group_thousands() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1000), "1,000");
        assert_eq!(format_number(1_234_567), "1,234,567");
    }

    #[test]
    fn durations_drop_zero_parts_and_never_go_empty() {
        assert_eq!(format_duration(0.0), "0s");
        assert_eq!(format_duration(0.9), "0s");
        assert_eq!(format_duration(61.0), "1m 1s");
        assert_eq!(format_duration(3600.0), "1h");
        assert_eq!(format_duration(90061.0), "1d 1h 1m 1s");
        assert_eq!(format_duration(-5.0), "0s");
        assert_eq!(format_duration(f64::NAN), "0s");
    }

    #[test]
    fn eta_is_unknown_until_a_row_has_been_timed() {
        assert_eq!(format_eta(500, 0.0), "unknown");
        assert_eq!(format_eta(500, 0.5), "4m 10s");
    }
}
