use crate::config::Config;
use crate::inbound::client::ReportClient;
use crate::outbound::client::{action::ActionClient, auth::AuthClient};
use anyhow::Context;
use chrono::Utc;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use tracing::info;
use tracing_subscriber::{
    Registry, filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt,
};

const LOG_DIR: &str = "log";

pub struct SetupResult {
    pub existing_ids: HashSet<String>,
    pub action_client: Option<ActionClient>,
    pub files_to_process: Vec<(PathBuf, String)>,
}

pub fn setup_logging(only_parse: bool, log_level: tracing::Level) -> anyhow::Result<()> {
    std::fs::create_dir_all(LOG_DIR)
        .with_context(|| format!("Failed to create log directory: {}", LOG_DIR))?;
    let timestamp_str = Utc::now().format("%Y-%m-%d_%H-%M-%S");
    let log_file_path = format!("{}/importer_{}.log", LOG_DIR, timestamp_str);
    let log_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_file_path)
        .with_context(|| format!("Failed to open log file: {}", log_file_path))?;
    let level_filter = if only_parse {
        LevelFilter::WARN
    } else {
        LevelFilter::from_level(log_level)
    };
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
    if only_parse {
        info!("Parse-only mode: will skip API calls");
    }
    info!("Configuration loaded successfully");
    Ok(())
}

pub async fn setup_auth_and_existing_ids(
    config: &Config,
    only_parse: bool,
) -> anyhow::Result<(String, HashSet<String>)> {
    if only_parse {
        return Ok((String::new(), HashSet::new()));
    }
    let auth_client = AuthClient::new(config.clone());
    let token = auth_client
        .get_valid_token()
        .await
        .context("Failed to authenticate with Halo API")?;
    info!("Authentication successful");
    let report_client = ReportClient::new(config.clone(), token.clone());
    let ids = report_client
        .get_existing_action_ids()
        .await
        .context("Failed to fetch existing action IDs from report")?;
    info!("Found {} existing action IDs to skip", ids.len());
    Ok((token, ids))
}

pub fn discover_files(input_path: &str) -> anyhow::Result<Vec<(PathBuf, String)>> {
    let input_dir = Path::new(input_path);
    if !input_dir.exists() {
        anyhow::bail!("Input directory '{}' does not exist", input_path);
    }
    let mut files_to_process = Vec::new();
    for entry in std::fs::read_dir(input_dir)
        .with_context(|| format!("Failed to read input directory: {}", input_path))?
    {
        let entry = entry
            .with_context(|| format!("Failed to read entry in input directory: {}", input_path))?;
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
    Ok(files_to_process)
}

pub async fn setup(
    config: &Config,
    only_parse: bool,
    input_path: &str,
) -> anyhow::Result<SetupResult> {
    let (auth_token, existing_ids) = setup_auth_and_existing_ids(config, only_parse).await?;
    let action_client = if only_parse {
        None
    } else {
        Some(ActionClient::new(config.clone(), auth_token))
    };
    let files_to_process = discover_files(input_path)?;
    Ok(SetupResult {
        existing_ids,
        action_client,
        files_to_process,
    })
}
