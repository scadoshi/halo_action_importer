use tracing::{info, warn};

pub struct ImportSummary {
    pub total_processed: usize,
    pub total_imported: usize,
    pub total_skipped: usize,
    pub total_failed: usize,
    pub skipped_files: Vec<String>,
}

pub fn log_summary(summary: ImportSummary) {
    info!("=== Import Summary ===");
    info!("Total actions processed: {}", summary.total_processed);
    info!("Actions skipped (already exist): {}", summary.total_skipped);
    info!("Actions successfully imported: {}", summary.total_imported);
    info!("Actions failed to import: {}", summary.total_failed);
    if !summary.skipped_files.is_empty() {
        warn!("Files that could not be read: {}", summary.skipped_files.len());
    }
}

