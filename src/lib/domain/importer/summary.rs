use tracing::{info, warn};

fn format_number(n: usize) -> String {
    let s = n.to_string();
    let mut result = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, &ch) in chars.iter().enumerate() {
        if i > 0 && (chars.len() - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(ch);
    }
    result
}

pub struct ImportSummary {
    pub total_processed: usize,
    pub total_imported: usize,
    pub total_skipped: usize,
    pub total_failed: usize,
    pub skipped_files: Vec<String>,
    pub total_runtime_secs: f64,
    pub sheet_times: Vec<f64>,
}

pub fn log_summary(summary: ImportSummary, only_parse: bool) {
    info!("=== Import Summary ===");
    info!(
        "Total actions processed: {}",
        format_number(summary.total_processed)
    );
    info!(
        "Actions skipped (already exist): {}",
        format_number(summary.total_skipped)
    );
    info!(
        "Actions successfully imported: {}",
        format_number(summary.total_imported)
    );
    info!(
        "Actions failed to import: {}",
        format_number(summary.total_failed)
    );
    if !summary.skipped_files.is_empty() {
        warn!(
            "Files that could not be read: {}",
            format_number(summary.skipped_files.len())
        );
    }
    if only_parse && summary.total_failed == 0 && summary.skipped_files.is_empty() {
        let successful = summary.total_imported + summary.total_skipped;
        info!(
            "Success: {}/{} actions parsed successfully",
            format_number(successful),
            format_number(summary.total_processed)
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
