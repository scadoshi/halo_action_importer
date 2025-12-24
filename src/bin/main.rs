use anyhow::Context;
use halo_action_importer::{
    config::Config,
    domain::importer::{
        ImportSummary, SetupResult, log_summary, process_csv_file, process_excel_file, setup,
    },
};
use std::ffi::OsStr;
use std::time::Instant;
use tracing::{error, info};

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

const INPUT_PATH: &str = "input";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let only_parse = std::env::args().any(|arg| arg == "--only-parse" || arg == "--op");
    let reverse = std::env::args().any(|arg| arg == "--reverse" || arg == "--rev");
    let half = std::env::args().any(|arg| arg == "--half");
    let config =
        Config::from_env().context("Failed to load configuration from environment variables")?;

    setup::setup_logging(only_parse, config.log_level)?;

    let SetupResult {
        existing_ids,
        action_client,
        files_to_process,
        auth_client: _,
    } = setup::setup(&config, only_parse, reverse, half, INPUT_PATH).await?;

    let total_sheets = files_to_process.len();
    if half {
        if reverse {
            info!(
                "Running in half mode (bottom half) with reverse: processing {} file(s) from bottom",
                format_number(total_sheets)
            );
        } else {
            info!(
                "Running in half mode (top half): processing {} file(s) from top",
                format_number(total_sheets)
            );
        }
    } else if reverse {
        info!("Running in reverse mode: processing files from bottom to top");
    }
    if only_parse {
        if reverse {
            info!(
                "Starting parse-only import of {} file(s) in reverse order",
                format_number(total_sheets)
            );
        } else {
            info!(
                "Starting parse-only import of {} file(s)",
                format_number(total_sheets)
            );
        }
    } else if reverse {
        info!(
            "Starting import of {} file(s) in reverse order",
            format_number(total_sheets)
        );
    } else {
        info!("Starting import of {} file(s)", format_number(total_sheets));
    }
    let mut total_actions_processed = 0;
    let mut total_actions_imported = 0;
    let mut total_actions_skipped = 0;
    let mut skipped_files: Vec<String> = Vec::new();
    let mut failed_imports: Vec<(String, String)> = Vec::new();
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
            skipped_files,
            total_runtime_secs: total_runtime,
            sheet_times,
        },
        only_parse,
    );

    Ok(())
}
