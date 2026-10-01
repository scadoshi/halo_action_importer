use anyhow::Context;
use clap::Parser;
use halo_action_importer::{
    cli::Cli,
    config::Config,
    domain::importer::{
        FailedAction, ImportSummary, SetupResult, format::format_number, log_summary,
        process_csv_file, process_excel_file, setup, write_retry_csv, write_summary_json,
    },
};
use std::ffi::OsStr;
use std::time::Instant;
use tracing::{error, info};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let Cli {
        only_parse,
        cache_only,
        input_path,
        batch_size,
    } = Cli::parse();
    let input_path = input_path.as_str();
    let batch_size = usize::from(batch_size);
    let config =
        Config::from_env().context("Failed to load configuration from environment variables")?;

    let log_dir_path = setup::setup_logging(only_parse, config.log_level)?;

    let SetupResult {
        existing_ids,
        action_client,
        files_to_process,
        auth_client: _,
    } = setup::setup(&config, only_parse, cache_only, input_path).await?;

    let total_sheets = files_to_process.len();
    info!("Processing files from directory: {}", input_path);
    if cache_only {
        info!("Cache mode: using cached IDs only, skipping report fetching");
    }
    if batch_size > 1 {
        info!(
            "Batch mode enabled: posting {} actions per request",
            format_number(batch_size)
        );
    }
    if only_parse {
        info!(
            "Starting parse-only import of {} file(s)",
            format_number(total_sheets)
        );
    } else {
        info!("Starting import of {} file(s)", format_number(total_sheets));
    }
    let mut total_actions_processed = 0;
    let mut total_actions_imported = 0;
    let mut total_actions_skipped = 0;
    let mut skipped_files: Vec<String> = Vec::new();
    let mut failed_imports: Vec<FailedAction> = Vec::new();
    let mut sheet_times: Vec<f64> = Vec::new();
    let start_time = Instant::now();

    for (sheet_num, (file_path, file_name)) in files_to_process.iter().enumerate() {
        let sheet_number = sheet_num + 1;
        if let Some(ext) = file_path.extension().and_then(OsStr::to_str) {
            let ext_lower = ext.to_lowercase();
            let result = if ext_lower == "csv" {
                process_csv_file(
                    file_path,
                    &existing_ids,
                    action_client.as_ref(),
                    &mut sheet_times,
                    file_name,
                    sheet_number,
                    total_sheets,
                    only_parse,
                    batch_size,
                )
                .await
            } else if ext_lower == "xlsx" || ext_lower == "xls" {
                process_excel_file(
                    file_path,
                    &existing_ids,
                    action_client.as_ref(),
                    &mut sheet_times,
                    sheet_number,
                    total_sheets,
                    only_parse,
                    batch_size,
                )
                .await
            } else {
                continue;
            };

            match result {
                Ok(stats) => {
                    total_actions_processed += stats.processed;
                    total_actions_imported += stats.imported;
                    total_actions_skipped += stats.skipped;
                    failed_imports.extend(stats.failed);
                }
                Err(e) => {
                    error!("Failed to read file {:?}: {}", file_name, e);
                    skipped_files.push(format!("{:?}: {}", file_name, e));
                }
            }
        }
    }

    let total_runtime = start_time.elapsed().as_secs_f64();
    log_summary(
        ImportSummary {
            total_processed: total_actions_processed,
            total_imported: total_actions_imported,
            total_skipped: total_actions_skipped,
            total_failed: failed_imports.len(),
            skipped_files: skipped_files.clone(),
            total_runtime_secs: total_runtime,
            sheet_times: sheet_times.clone(),
        },
        only_parse,
    );

    // Write retry.csv and summary.json
    if !failed_imports.is_empty() {
        write_retry_csv(&log_dir_path, &failed_imports)?;
    }

    write_summary_json(
        &log_dir_path,
        total_actions_processed,
        total_actions_imported,
        total_actions_skipped,
        &failed_imports,
        total_runtime,
        &skipped_files,
        total_sheets,
    )?;

    Ok(())
}
