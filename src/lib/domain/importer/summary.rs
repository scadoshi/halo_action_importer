//! The summary a run logs at the end, and its exit code.

use tracing::{info, warn};

use super::format;

/// The counts and timings the end-of-run summary logs.
pub struct ImportSummary {
    /// Rows read across every file.
    pub total_processed: usize,
    /// Actions Halo took.
    pub total_imported: usize,
    /// Actions not posted: already in Halo, or for a missing ticket.
    pub total_skipped: usize,
    /// Actions Halo did not take, plus rows that did not parse.
    pub total_failed: usize,
    /// Each file that could not be read, with the reason.
    pub skipped_files: Vec<String>,
    /// The whole run, in seconds.
    pub total_runtime_secs: f64,
    /// Each file's time, in seconds.
    pub sheet_times: Vec<f64>,
}

/// The process exit code for a run: 1 when any action failed or any file
/// could not be read, 0 otherwise.
pub fn exit_code(summary: &ImportSummary) -> u8 {
    if summary.total_failed > 0 || !summary.skipped_files.is_empty() {
        1
    } else {
        0
    }
}

/// Logs the counts and, when any rows were read, the timings.
pub fn log_summary(summary: ImportSummary, only_parse: bool) {
    info!("=== Import Summary ===");
    info!(
        "Total actions processed: {}",
        format::format_number(summary.total_processed)
    );
    info!(
        "Actions skipped (already exist): {}",
        format::format_number(summary.total_skipped)
    );
    info!(
        "Actions successfully imported: {}",
        format::format_number(summary.total_imported)
    );
    info!(
        "Actions failed to import: {}",
        format::format_number(summary.total_failed)
    );
    if !summary.skipped_files.is_empty() {
        warn!(
            "Files that could not be read: {}",
            format::format_number(summary.skipped_files.len())
        );
    }
    if only_parse && summary.total_failed == 0 && summary.skipped_files.is_empty() {
        let successful = summary.total_imported + summary.total_skipped;
        info!(
            "Success: {}/{} actions parsed successfully",
            format::format_number(successful),
            format::format_number(summary.total_processed)
        );
    }
    if summary.total_processed > 0 {
        let total_runtime = summary.total_runtime_secs;
        let time_per_entry = total_runtime / summary.total_processed as f64;
        let entries_per_minute = if total_runtime > 0.0 {
            (summary.total_processed as f64 / total_runtime) * 60.0
        } else {
            0.0
        };
        let avg_sheet_time = if !summary.sheet_times.is_empty() {
            summary.sheet_times.iter().sum::<f64>() / summary.sheet_times.len() as f64
        } else {
            0.0
        };
        info!("=== Performance Stats ===");
        info!(
            "Total runtime: {:.2}s ({:.2}m)",
            total_runtime,
            total_runtime / 60.0
        );
        info!("Time per entry: {:.3}s", time_per_entry);
        info!("Entries per minute: {:.1}", entries_per_minute);
        if !summary.sheet_times.is_empty() {
            info!("Average time per sheet: {:.2}s", avg_sheet_time);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(total_failed: usize, skipped_files: &[&str]) -> ImportSummary {
        ImportSummary {
            total_processed: 10,
            total_imported: 10 - total_failed,
            total_skipped: 0,
            total_failed,
            skipped_files: skipped_files.iter().map(|f| (*f).to_string()).collect(),
            total_runtime_secs: 1.0,
            sheet_times: Vec::new(),
        }
    }

    #[test]
    fn a_run_exits_one_when_an_action_failed_or_a_file_could_not_be_read() {
        assert_eq!(exit_code(&summary(0, &[])), 0);
        assert_eq!(exit_code(&summary(1, &[])), 1);
        assert_eq!(exit_code(&summary(0, &["a.csv: unreadable"])), 1);
    }
}
