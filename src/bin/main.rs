use std::process::ExitCode;
use std::time::Instant;

use anyhow::Context;
use clap::Parser;
use halo_action_importer::cli::Cli;
use halo_action_importer::config::Config;
use halo_action_importer::domain::importer::format::format_number;
use halo_action_importer::domain::importer::{
    Cache, ImportSummary, ProcessConfig, Run, SetupOptions, SetupResult, exit_code, log_summary,
    setup, write_retry_csv, write_summary_json,
};
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    let Cli {
        only_parse,
        sync_from_halo,
        input_path,
        batch_size,
    } = Cli::parse();
    let input_path = input_path.as_str();
    let batch_size = usize::from(batch_size);
    let config =
        Config::from_env().context("Failed to load configuration from environment variables")?;

    let log_dir_path = setup::setup_logging(only_parse, config.log_level)?;
    let cache = Cache::default();

    let SetupResult {
        existing_ids,
        action_client,
        files_to_process,
        auth_client: _,
    } = setup::setup(
        &config,
        SetupOptions {
            only_parse,
            sync_from_halo,
        },
        &cache,
        input_path,
    )
    .await?;

    let total_sheets = files_to_process.len();
    info!("Processing files from directory: {}", input_path);
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
    let start_time = Instant::now();
    let run = Run::new(
        ProcessConfig {
            action_client: action_client.as_ref(),
            cache: &cache,
            only_parse,
            batch_size,
        },
        existing_ids,
        total_sheets,
    );
    let totals = run.process_files(&files_to_process).await;

    let total_runtime = start_time.elapsed().as_secs_f64();
    let summary = ImportSummary {
        total_processed: totals.processed,
        total_imported: totals.imported,
        total_skipped: totals.skipped,
        total_failed: totals.failed.len(),
        skipped_files: totals.skipped_files.clone(),
        total_runtime_secs: total_runtime,
        sheet_times: totals.sheet_times.clone(),
    };
    let code = exit_code(&summary);
    log_summary(summary, only_parse);

    // Write retry.csv and summary.json
    if !totals.failed.is_empty() {
        write_retry_csv(&log_dir_path, &totals.failed)?;
    }

    write_summary_json(&log_dir_path, &totals, total_runtime)?;

    Ok(ExitCode::from(code))
}
