use crate::inbound::file::{Reader, csv::Csv, excel::Excel};
use crate::outbound::client::action::ActionClient;
use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;
use tracing::{error, info};

pub struct ProcessingStats {
    pub processed: usize,
    pub imported: usize,
    pub skipped: usize,
    pub failed: Vec<(String, String)>,
}

pub async fn process_csv_file(
    file_path: &Path,
    existing_ids: &HashSet<String>,
    action_client: Option<&ActionClient>,
    sheet_times: &mut Vec<f64>,
    file_name: &str,
    sheet_number: usize,
    total_sheets: usize,
    only_parse: bool,
) -> anyhow::Result<ProcessingStats> {
    let mut iter = <Reader as Csv>::csv_action_iter(file_path)?;
    let total_rows = iter.total_rows();
    let mut processed = 0;
    let mut imported = 0;
    let mut skipped = 0;
    let mut failed = Vec::new();
    let sheet_start = Instant::now();
    let mut row_times: Vec<f64> = Vec::new();
    let mut last_progress_log = Instant::now();
    if let Some(total) = total_rows {
        info!(
            "Processing sheet {} of {}: CSV file '{}' ({} rows)",
            sheet_number, total_sheets, file_name, total
        );
    } else {
        info!(
            "Processing sheet {} of {}: CSV file '{}'",
            sheet_number, total_sheets, file_name
        );
    }
    while let Some(action_result) = iter.next() {
        let row_start = Instant::now();
        let action = match action_result {
            Ok(a) => a,
            Err(e) => {
                let error_msg = format!(
                    "Failed to deserialize row in CSV file '{}': {}",
                    file_name, e
                );
                error!("{}", error_msg);
                failed.push(("unknown".to_string(), error_msg.clone()));
                continue;
            }
        };
        processed += 1;
        let action_id = action.action_id().to_string();
        if only_parse {
            if existing_ids.contains(&action_id) {
                skipped += 1;
            } else {
                imported += 1;
            }
        } else {
            if existing_ids.contains(&action_id) {
                skipped += 1;
                info!("Skipped: action ID {} - already exists", action_id);
            } else {
                match action_client.unwrap().post_action_object(action).await {
                    Ok(_) => {
                        imported += 1;
                        info!("Success: imported action ID {}", action_id);
                    }
                    Err(e) => {
                        let error_msg = format!("Failed to import action ID {}: {}", action_id, e);
                        error!("{}", error_msg);
                        failed.push((action_id, error_msg.clone()));
                    }
                }
            }
        }
        let row_duration = row_start.elapsed().as_secs_f64();
        row_times.push(row_duration);
        if last_progress_log.elapsed().as_secs() >= 60 || processed % 300 == 0 {
            log_progress(
                sheet_number,
                total_sheets,
                file_name,
                None,
                processed,
                total_rows,
                imported,
                skipped,
                &row_times,
            );
            last_progress_log = Instant::now();
        }
    }
    let sheet_duration = sheet_start.elapsed().as_secs_f64();
    sheet_times.push(sheet_duration);
    let avg_sheet_time = sheet_times.iter().sum::<f64>() / sheet_times.len() as f64;
    info!(
        "Completed sheet {} of {}: CSV file '{}' | {} processed, {} imported, {} skipped in {:.1}s | avg sheet time: {:.1}s",
        sheet_number,
        total_sheets,
        file_name,
        processed,
        imported,
        skipped,
        sheet_duration,
        avg_sheet_time
    );
    Ok(ProcessingStats {
        processed,
        imported,
        skipped,
        failed,
    })
}

pub async fn process_excel_file(
    file_path: &Path,
    existing_ids: &HashSet<String>,
    action_client: Option<&ActionClient>,
    sheet_times: &mut Vec<f64>,
    sheet_number: usize,
    total_sheets: usize,
    only_parse: bool,
) -> anyhow::Result<ProcessingStats> {
    let file_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown file");
    let mut iter = <Reader as Excel>::excel_action_iter(file_path)?;
    let total_rows = iter.total_rows();
    let sheet_name = iter.sheet_name().to_string();
    let mut processed = 0;
    let mut imported = 0;
    let mut skipped = 0;
    let mut failed = Vec::new();
    let sheet_start = Instant::now();
    let mut row_times: Vec<f64> = Vec::new();
    let mut last_progress_log = Instant::now();
    info!(
        "Processing sheet {} of {}: Excel file '{}', sheet '{}' ({} rows)",
        sheet_number, total_sheets, file_name, sheet_name, total_rows
    );
    while let Some(action_result) = iter.next() {
        let row_start = Instant::now();
        let action = match action_result {
            Ok(a) => a,
            Err(e) => {
                let error_msg = format!(
                    "Failed to deserialize row in Excel file '{}', sheet '{}': {}",
                    file_name, sheet_name, e
                );
                error!("{}", error_msg);
                failed.push(("unknown".to_string(), error_msg.clone()));
                continue;
            }
        };
        processed += 1;
        let action_id = action.action_id().to_string();
        if only_parse {
            if existing_ids.contains(&action_id) {
                skipped += 1;
            } else {
                imported += 1;
            }
        } else {
            if existing_ids.contains(&action_id) {
                skipped += 1;
                info!("Skipped: action ID {} - already exists", action_id);
            } else {
                match action_client.unwrap().post_action_object(action).await {
                    Ok(_) => {
                        imported += 1;
                        info!("Success: imported action ID {}", action_id);
                    }
                    Err(e) => {
                        let error_msg = format!("Failed to import action ID {}: {}", action_id, e);
                        error!("{}", error_msg);
                        failed.push((action_id, error_msg.clone()));
                    }
                }
            }
        }
        let row_duration = row_start.elapsed().as_secs_f64();
        row_times.push(row_duration);
        if last_progress_log.elapsed().as_secs() >= 60 || processed % 300 == 0 {
            log_progress(
                sheet_number,
                total_sheets,
                file_name,
                Some(&sheet_name),
                processed,
                Some(total_rows),
                imported,
                skipped,
                &row_times,
            );
            last_progress_log = Instant::now();
        }
    }
    let sheet_duration = sheet_start.elapsed().as_secs_f64();
    sheet_times.push(sheet_duration);
    let avg_sheet_time = sheet_times.iter().sum::<f64>() / sheet_times.len() as f64;
    info!(
        "Completed sheet {} of {}: Excel file '{}', sheet '{}' | {} processed, {} imported, {} skipped in {:.1}s | avg sheet time: {:.1}s",
        sheet_number,
        total_sheets,
        file_name,
        sheet_name,
        processed,
        imported,
        skipped,
        sheet_duration,
        avg_sheet_time
    );
    Ok(ProcessingStats {
        processed,
        imported,
        skipped,
        failed,
    })
}

fn log_progress(
    sheet_number: usize,
    total_sheets: usize,
    file_name: &str,
    sheet_name: Option<&str>,
    processed: usize,
    total_rows: Option<usize>,
    imported: usize,
    skipped: usize,
    row_times: &[f64],
) {
    let avg_row_time = if row_times.is_empty() {
        0.0
    } else {
        row_times.iter().sum::<f64>() / row_times.len() as f64
    };
    if let Some(total) = total_rows {
        let remaining = total.saturating_sub(processed);
        let estimated_remaining = avg_row_time * remaining as f64;
        let progress_pct = if total > 0 {
            (processed as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        if let Some(sheet) = sheet_name {
            info!(
                "Progress [sheet {} of {}: '{}' - sheet '{}']: {}/{} rows ({:.1}%), {} imported, {} skipped | avg {:.2}s/row | est. remaining: {:.1}s",
                sheet_number,
                total_sheets,
                file_name,
                sheet,
                processed,
                total,
                progress_pct,
                imported,
                skipped,
                avg_row_time,
                estimated_remaining
            );
        } else {
            info!(
                "Progress [sheet {} of {}: '{}']: {}/{} rows ({:.1}%), {} imported, {} skipped | avg {:.2}s/row | est. remaining: {:.1}s",
                sheet_number,
                total_sheets,
                file_name,
                processed,
                total,
                progress_pct,
                imported,
                skipped,
                avg_row_time,
                estimated_remaining
            );
        }
    } else {
        info!(
            "Progress [sheet {} of {}: '{}']: processed {} rows, {} imported, {} skipped | avg {:.2}s/row",
            sheet_number, total_sheets, file_name, processed, imported, skipped, avg_row_time
        );
    }
}
