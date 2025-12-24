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

struct ProcessConfig<'a> {
    existing_ids: &'a HashSet<String>,
    action_client: Option<&'a ActionClient>,
    sheet_times: &'a mut Vec<f64>,
    file_name: &'a str,
    sheet_number: usize,
    total_sheets: usize,
    only_parse: bool,
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
    let config = ProcessConfig {
        existing_ids,
        action_client,
        sheet_times,
        file_name,
        sheet_number,
        total_sheets,
        only_parse,
    };
    let iter = <Reader as Csv>::csv_action_iter(file_path)?;
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
            config.sheet_number,
            config.total_sheets,
            config.file_name,
            format_number(total)
        );
    } else {
        info!(
            "Processing sheet {} of {}: CSV file '{}'",
            config.sheet_number, config.total_sheets, config.file_name
        );
    }
    for action_result in iter {
        let row_start = Instant::now();
        let action = match action_result {
            Ok(a) => a,
            Err(e) => {
                let error_msg = format!(
                    "Failed to deserialize row in CSV file '{}': {}",
                    config.file_name, e
                );
                error!("{}", error_msg);
                failed.push(("unknown".to_string(), error_msg.clone()));
                continue;
            }
        };
        processed += 1;
        let action_id = action.action_id().to_string();
        if config.only_parse {
            if config.existing_ids.contains(&action_id) {
                skipped += 1;
            } else {
                imported += 1;
            }
        } else if config.existing_ids.contains(&action_id) {
                skipped += 1;
                info!("Skipped: action ID {} - already exists", action_id);
        } else if let Some(client) = config.action_client {
            match client.post_action_object(action.clone()).await {
                    Ok(_) => {
                        imported += 1;
                    info!(
                        "Success: imported action ID {} (ticket ID: {})",
                        action_id, action.ticket_id
                    );
                    }
                    Err(e) => {
                        let error_msg = format!("Failed to import action ID {}: {}", action_id, e);
                        error!("{}", error_msg);
                        failed.push((action_id, error_msg.clone()));
                    }
                }
        } else {
            let error_msg = format!("Action client not available for action ID {}", action_id);
            error!("{}", error_msg);
            failed.push((action_id, error_msg.clone()));
        }
        let row_duration = row_start.elapsed().as_secs_f64();
        row_times.push(row_duration);
        let should_log_progress = if only_parse {
            last_progress_log.elapsed().as_secs() >= 5 || processed % 10_000 == 0
        } else {
            last_progress_log.elapsed().as_secs() >= 60 || processed % 100 == 0
        };
        if should_log_progress {
            log_progress(ProgressParams {
                sheet_number: config.sheet_number,
                total_sheets: config.total_sheets,
                file_name: config.file_name,
                sheet_name: None,
                processed,
                total_rows,
                imported,
                skipped,
                row_times: &row_times,
            });
            last_progress_log = Instant::now();
        }
    }
    let sheet_duration = sheet_start.elapsed().as_secs_f64();
    config.sheet_times.push(sheet_duration);
    let avg_sheet_time = config.sheet_times.iter().sum::<f64>() / config.sheet_times.len() as f64;
    info!(
        "Completed sheet {} of {}: CSV file '{}' | {} processed, {} imported, {} skipped in {:.1}s | avg sheet time: {:.1}s",
        config.sheet_number,
        config.total_sheets,
        config.file_name,
        format_number(processed),
        format_number(imported),
        format_number(skipped),
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
    let config = ProcessConfig {
        existing_ids,
        action_client,
        sheet_times,
        file_name: file_path
        .file_name()
        .and_then(|n| n.to_str())
            .unwrap_or("unknown file"),
        sheet_number,
        total_sheets,
        only_parse,
    };
    let iter = <Reader as Excel>::excel_action_iter(file_path)?;
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
        config.sheet_number,
        config.total_sheets,
        config.file_name,
        sheet_name,
        format_number(total_rows)
    );
    for action_result in iter {
        let row_start = Instant::now();
        let action = match action_result {
            Ok(a) => a,
            Err(e) => {
                let error_msg = format!(
                    "Failed to deserialize row in Excel file '{}', sheet '{}': {}",
                    config.file_name, sheet_name, e
                );
                error!("{}", error_msg);
                failed.push(("unknown".to_string(), error_msg.clone()));
                continue;
            }
        };
        processed += 1;
        let action_id = action.action_id().to_string();
        if config.only_parse {
            if config.existing_ids.contains(&action_id) {
                skipped += 1;
            } else {
                imported += 1;
            }
        } else if config.existing_ids.contains(&action_id) {
                skipped += 1;
                info!("Skipped: action ID {} - already exists", action_id);
        } else if let Some(client) = config.action_client {
            match client.post_action_object(action.clone()).await {
                    Ok(_) => {
                        imported += 1;
                    info!(
                        "Success: imported action ID {} (ticket ID: {})",
                        action_id, action.ticket_id
                    );
                    }
                    Err(e) => {
                        let error_msg = format!("Failed to import action ID {}: {}", action_id, e);
                        error!("{}", error_msg);
                        failed.push((action_id, error_msg.clone()));
                    }
                }
        } else {
            let error_msg = format!("Action client not available for action ID {}", action_id);
            error!("{}", error_msg);
            failed.push((action_id, error_msg.clone()));
        }
        let row_duration = row_start.elapsed().as_secs_f64();
        row_times.push(row_duration);
        if last_progress_log.elapsed().as_secs() >= 60 || processed % 300 == 0 {
            log_progress(ProgressParams {
                sheet_number: config.sheet_number,
                total_sheets: config.total_sheets,
                file_name: config.file_name,
                sheet_name: Some(&sheet_name),
                processed,
                total_rows: Some(total_rows),
                imported,
                skipped,
                row_times: &row_times,
            });
            last_progress_log = Instant::now();
        }
    }
    let sheet_duration = sheet_start.elapsed().as_secs_f64();
    config.sheet_times.push(sheet_duration);
    let avg_sheet_time = config.sheet_times.iter().sum::<f64>() / config.sheet_times.len() as f64;
    info!(
        "Completed sheet {} of {}: Excel file '{}', sheet '{}' | {} processed, {} imported, {} skipped in {:.1}s | avg sheet time: {:.1}s",
        config.sheet_number,
        config.total_sheets,
        config.file_name,
        sheet_name,
        format_number(processed),
        format_number(imported),
        format_number(skipped),
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

struct ProgressParams<'a> {
    sheet_number: usize,
    total_sheets: usize,
    file_name: &'a str,
    sheet_name: Option<&'a str>,
    processed: usize,
    total_rows: Option<usize>,
    imported: usize,
    skipped: usize,
    row_times: &'a [f64],
}

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

fn format_duration(seconds: f64) -> String {
    let total_seconds = seconds as u64;
    let days = total_seconds / 86400;
    let hours = (total_seconds % 86400) / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let secs = total_seconds % 60;

    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{}d", days));
    }
    if hours > 0 {
        parts.push(format!("{}h", hours));
    }
    if minutes > 0 {
        parts.push(format!("{}m", minutes));
    }
    if secs > 0 || parts.is_empty() {
        parts.push(format!("{}s", secs));
    }

    parts.join(" ")
}

fn log_progress(params: ProgressParams<'_>) {
    let avg_row_time = if params.row_times.is_empty() {
        0.0
    } else {
        params.row_times.iter().sum::<f64>() / params.row_times.len() as f64
    };
    if let Some(total) = params.total_rows {
        let remaining = total.saturating_sub(params.processed);
        let estimated_remaining = avg_row_time * remaining as f64;
        let remaining_formatted = format_duration(estimated_remaining);
        let progress_pct = if total > 0 {
            (params.processed as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        if let Some(sheet) = params.sheet_name {
            info!(
                "Progress [sheet {} of {}: '{}' - sheet '{}']: {}/{} rows ({:.1}%), {} imported, {} skipped | avg {:.2}s/row | est. remaining: {}",
                params.sheet_number,
                params.total_sheets,
                params.file_name,
                sheet,
                format_number(params.processed),
                format_number(total),
                progress_pct,
                format_number(params.imported),
                format_number(params.skipped),
                avg_row_time,
                remaining_formatted
            );
        } else {
            info!(
                "Progress [sheet {} of {}: '{}']: {}/{} rows ({:.1}%), {} imported, {} skipped | avg {:.2}s/row | est. remaining: {}",
                params.sheet_number,
                params.total_sheets,
                params.file_name,
                format_number(params.processed),
                format_number(total),
                progress_pct,
                format_number(params.imported),
                format_number(params.skipped),
                avg_row_time,
                remaining_formatted
            );
        }
    } else {
        info!(
            "Progress [sheet {} of {}: '{}']: processed {} rows, {} imported, {} skipped | avg {:.2}s/row",
            params.sheet_number,
            params.total_sheets,
            params.file_name,
            format_number(params.processed),
            format_number(params.imported),
            format_number(params.skipped),
            avg_row_time
        );
    }
}
