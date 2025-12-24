use anyhow::Context;
use chrono::Utc;
use halo_action_importer::{
    config::Config,
    inbound::{
        client::ReportClient,
        file::{Reader, csv::Csv, excel::Excel},
    },
    outbound::client::{action::ActionClient, auth::AuthClient},
};
use std::{ffi::OsStr, fs::OpenOptions, path::Path, time::Instant};
use tracing::{error, info, warn};
use tracing_subscriber::{
    Registry, filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt,
};

const INPUT_PATH: &str = "input";
const LOG_DIR: &str = "log";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load configuration: {}", e);
            return Err(e.context("Failed to load configuration from environment variables"));
        }
    };

    std::fs::create_dir_all(LOG_DIR)
        .with_context(|| format!("failed to create log directory: {}", LOG_DIR))?;
    let timestamp_str = Utc::now().format("%Y-%m-%d_%H-%M-%S");
    let log_file_path = format!("{}/importer_{}.log", LOG_DIR, timestamp_str);
    let log_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_file_path)
        .with_context(|| format!("failed to open log file: {}", log_file_path))?;
    let level_filter = LevelFilter::from_level(config.log_level);
    Registry::default()
        .with(level_filter)
        .with(
            tracing_subscriber::fmt::Layer::default()
                .with_writer(std::sync::Mutex::new(log_file))
                .with_ansi(false),
        )
        .with(tracing_subscriber::fmt::Layer::default().with_writer(std::io::stdout))
        .init();
    info!("Starting Halo action importer");
    info!("Configuration loaded successfully");
    let auth_client = AuthClient::new(config.clone());
    let auth_token = match auth_client.get_valid_token().await {
        Ok(t) => t,
        Err(e) => {
            error!("Authentication failed: {}", e);
            return Err(e.context("Failed to authenticate with Halo API"));
        }
    };
    info!("Authentication successful");
    let report_client = ReportClient::new(config.clone(), auth_token.clone());
    let existing_ids = match report_client.get_existing_action_ids().await {
        Ok(ids) => ids,
        Err(e) => {
            error!("Failed to fetch existing action IDs: {}", e);
            return Err(e.context("Failed to fetch existing action IDs from report"));
        }
    };
    info!("Found {} existing action IDs to skip", existing_ids.len());
    let input_dir = Path::new(INPUT_PATH);
    if !input_dir.exists() {
        error!("Input directory '{}' does not exist", INPUT_PATH);
        anyhow::bail!("Input directory '{}' does not exist", INPUT_PATH);
    }
    let action_client = ActionClient::new(config, auth_token);
    let mut files_to_process: Vec<(std::path::PathBuf, String)> = Vec::new();
    for entry in std::fs::read_dir(input_dir)
        .with_context(|| format!("failed to read input directory: {}", INPUT_PATH))?
    {
        let entry = entry
            .with_context(|| format!("failed to read entry in input directory: {}", INPUT_PATH))?;
        let file_path = entry.path();
        if !file_path.is_file() {
            continue;
        }
        if let Some(ext) = file_path.extension().and_then(OsStr::to_str) {
            let ext_lower = ext.to_lowercase();
            if ext_lower == "csv" || ext_lower == "xlsx" || ext_lower == "xls" {
                let file_name = file_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                files_to_process.push((file_path, file_name));
            }
        }
    }
    let total_sheets = files_to_process.len();
    let mut total_actions_processed = 0;
    let mut total_actions_imported = 0;
    let mut total_actions_skipped = 0;
    let mut skipped_files: Vec<String> = Vec::new();
    let mut failed_imports: Vec<(String, String)> = Vec::new();
    let mut sheet_times: Vec<f64> = Vec::new();
    for (sheet_num, (file_path, file_name)) in files_to_process.iter().enumerate() {
        let sheet_number = sheet_num + 1;
        if let Some(ext) = file_path.extension().and_then(OsStr::to_str) {
            let ext_lower = ext.to_lowercase();
            if ext_lower == "csv" {
                match process_csv_file(
                    file_path,
                    &existing_ids,
                    &action_client,
                    &mut sheet_times,
                    &file_name,
                    sheet_number,
                    total_sheets,
                )
                .await
                {
                    Ok((processed, imported, skipped, failed)) => {
                        total_actions_processed += processed;
                        total_actions_imported += imported;
                        total_actions_skipped += skipped;
                        failed_imports.extend(failed);
                    }
                    Err(e) => {
                        warn!("Failed to read CSV file {:?}: {}", file_name, e);
                        skipped_files.push(format!("{:?}: {}", file_name, e));
                    }
                }
            } else if ext_lower == "xlsx" || ext_lower == "xls" {
                match process_excel_file(
                    file_path,
                    &existing_ids,
                    &action_client,
                    &mut sheet_times,
                    sheet_number,
                    total_sheets,
                )
                .await
                {
                    Ok((processed, imported, skipped, failed)) => {
                        total_actions_processed += processed;
                        total_actions_imported += imported;
                        total_actions_skipped += skipped;
                        failed_imports.extend(failed);
                    }
                    Err(e) => {
                        warn!("Failed to read Excel file {:?}: {}", file_name, e);
                        skipped_files.push(format!("{:?}: {}", file_name, e));
                    }
                }
            }
        }
    }
    info!("=== Import Summary ===");
    info!("Total actions processed: {}", total_actions_processed);
    info!("Actions skipped (already exist): {}", total_actions_skipped);
    info!("Actions successfully imported: {}", total_actions_imported);
    info!("Actions failed to import: {}", failed_imports.len());
    if !skipped_files.is_empty() {
        warn!("Files that could not be read: {}", skipped_files.len());
    }
    Ok(())
}

async fn process_csv_file(
    file_path: &Path,
    existing_ids: &std::collections::HashSet<String>,
    action_client: &ActionClient,
    sheet_times: &mut Vec<f64>,
    file_name: &str,
    sheet_number: usize,
    total_sheets: usize,
) -> anyhow::Result<(usize, usize, usize, Vec<(String, String)>)> {
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
        if existing_ids.contains(&action_id) {
            skipped += 1;
            info!("Skipped: action ID {} - already exists", action_id);
        } else {
            match action_client.post_action_object(action).await {
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
        let row_duration = row_start.elapsed().as_secs_f64();
        row_times.push(row_duration);
        if last_progress_log.elapsed().as_secs() >= 60 || processed % 300 == 0 {
            let avg_row_time = if row_times.is_empty() {
                0.0
            } else {
                row_times.iter().sum::<f64>() / row_times.len() as f64
            };
            let estimated_remaining = if let Some(total) = total_rows {
                let remaining = total.saturating_sub(processed);
                avg_row_time * remaining as f64
            } else {
                0.0
            };
            if let Some(total) = total_rows {
                let progress_pct = if total > 0 {
                    (processed as f64 / total as f64) * 100.0
                } else {
                    0.0
                };
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
            } else {
                info!(
                    "Progress [sheet {} of {}: '{}']: processed {} rows, {} imported, {} skipped | avg {:.2}s/row",
                    sheet_number,
                    total_sheets,
                    file_name,
                    processed,
                    imported,
                    skipped,
                    avg_row_time
                );
            }
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
    Ok((processed, imported, skipped, failed))
}

async fn process_excel_file(
    file_path: &Path,
    existing_ids: &std::collections::HashSet<String>,
    action_client: &ActionClient,
    sheet_times: &mut Vec<f64>,
    sheet_number: usize,
    total_sheets: usize,
) -> anyhow::Result<(usize, usize, usize, Vec<(String, String)>)> {
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
        if existing_ids.contains(&action_id) {
            skipped += 1;
            if skipped <= 10 {
                info!("Skipped: action ID {} - already exists", action_id);
            }
        } else {
            match action_client.post_action_object(action).await {
                Ok(_) => {
                    imported += 1;
                    if imported <= 10 {
                        info!("Success: imported action ID {}", action_id);
                    }
                }
                Err(e) => {
                    let error_msg = format!("Failed to import action ID {}: {}", action_id, e);
                    error!("{}", error_msg);
                    failed.push((action_id, error_msg.clone()));
                }
            }
        }
        let row_duration = row_start.elapsed().as_secs_f64();
        row_times.push(row_duration);
        if last_progress_log.elapsed().as_secs() >= 60 || processed % 300 == 0 {
            let avg_row_time = if row_times.is_empty() {
                0.0
            } else {
                row_times.iter().sum::<f64>() / row_times.len() as f64
            };
            let remaining_rows = total_rows.saturating_sub(processed);
            let estimated_remaining = avg_row_time * remaining_rows as f64;
            let progress_pct = if total_rows > 0 {
                (processed as f64 / total_rows as f64) * 100.0
            } else {
                0.0
            };
            info!(
                "Progress [sheet {} of {}: '{}' - sheet '{}']: {}/{} rows ({:.1}%), {} imported, {} skipped | avg {:.2}s/row | est. remaining: {:.1}s",
                sheet_number,
                total_sheets,
                file_name,
                sheet_name,
                processed,
                total_rows,
                progress_pct,
                imported,
                skipped,
                avg_row_time,
                estimated_remaining
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
    Ok((processed, imported, skipped, failed))
}
