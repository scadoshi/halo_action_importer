use crate::config::Config;
use crate::inbound::client::ReportClient;
use crate::outbound::client::{action::ActionClient, auth::AuthClient};
use anyhow::Context;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::info;
use tracing_subscriber::{
    Registry, filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt,
};

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

const LOG_DIR: &str = "log";
const CACHE_DIR: &str = "cache";
const CACHE_FILE: &str = "cache/existing_action_ids.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceCache {
    pub resource_id: String,
    pub action_ids: Vec<String>,
}

pub struct CacheData {
    pub action_ids: HashSet<String>,
    pub fetched_resources: HashSet<String>,
}

/// Read cached action IDs from JSON file
pub fn read_cached_ids() -> CacheData {
    let path = Path::new(CACHE_FILE);
    if !path.exists() {
        return CacheData {
            action_ids: HashSet::new(),
            fetched_resources: HashSet::new(),
        };
    }

    match std::fs::read_to_string(path) {
        Ok(contents) => match serde_json::from_str::<Vec<ResourceCache>>(&contents) {
            Ok(resources) => {
                let mut action_ids = HashSet::new();
                let mut fetched_resources = HashSet::new();
                for resource in resources {
                    fetched_resources.insert(resource.resource_id);
                    for id in resource.action_ids {
                        action_ids.insert(id);
                    }
                }
                CacheData {
                    action_ids,
                    fetched_resources,
                }
            }
            Err(_) => CacheData {
                action_ids: HashSet::new(),
                fetched_resources: HashSet::new(),
            },
        },
        Err(_) => CacheData {
            action_ids: HashSet::new(),
            fetched_resources: HashSet::new(),
        },
    }
}

/// Append a resource with its action IDs to the cache file
pub fn append_resource_to_cache(resource_id: &str, action_ids: &[String]) -> anyhow::Result<()> {
    if action_ids.is_empty() {
        return Ok(());
    }

    std::fs::create_dir_all(CACHE_DIR)
        .with_context(|| format!("Failed to create cache directory: {}", CACHE_DIR))?;

    let path = Path::new(CACHE_FILE);
    let mut resources: Vec<ResourceCache> = if path.exists() {
        match std::fs::read_to_string(path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };

    if let Some(existing) = resources.iter_mut().find(|r| r.resource_id == resource_id) {
        existing.action_ids.extend(action_ids.iter().cloned());
    } else {
        resources.push(ResourceCache {
            resource_id: resource_id.to_string(),
            action_ids: action_ids.to_vec(),
        });
    }

    let json = serde_json::to_string_pretty(&resources)
        .with_context(|| "Failed to serialize cache to JSON")?;

    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(CACHE_FILE)
        .with_context(|| format!("Failed to open cache file: {}", CACHE_FILE))?;

    file.write_all(json.as_bytes())?;
    Ok(())
}

/// Append action IDs for imported actions (no resource tracking)
pub fn append_imported_ids_to_cache(ids: &[String]) -> anyhow::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }

    std::fs::create_dir_all(CACHE_DIR)
        .with_context(|| format!("Failed to create cache directory: {}", CACHE_DIR))?;

    let path = Path::new(CACHE_FILE);
    let mut resources: Vec<ResourceCache> = if path.exists() {
        match std::fs::read_to_string(path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };

    const IMPORTED_RESOURCE_ID: &str = "_imported";
    if let Some(existing) = resources
        .iter_mut()
        .find(|r| r.resource_id == IMPORTED_RESOURCE_ID)
    {
        existing.action_ids.extend(ids.iter().cloned());
    } else {
        resources.push(ResourceCache {
            resource_id: IMPORTED_RESOURCE_ID.to_string(),
            action_ids: ids.to_vec(),
        });
    }

    let json = serde_json::to_string_pretty(&resources)
        .with_context(|| "Failed to serialize cache to JSON")?;

    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(CACHE_FILE)
        .with_context(|| format!("Failed to open cache file: {}", CACHE_FILE))?;

    file.write_all(json.as_bytes())?;
    Ok(())
}

pub struct SetupResult {
    pub existing_ids: HashSet<String>,
    pub action_client: Option<ActionClient>,
    pub files_to_process: Vec<(PathBuf, String)>,
    pub auth_client: Option<Arc<AuthClient>>,
}

pub fn setup_logging(only_parse: bool, log_level: tracing::Level) -> anyhow::Result<()> {
    std::fs::create_dir_all(LOG_DIR)
        .with_context(|| format!("Failed to create log directory: {}", LOG_DIR))?;
    let timestamp_str = Utc::now().format("%Y-%m-%d_%H-%M-%S");
    let log_file_path = format!("{}/{}.log", LOG_DIR, timestamp_str);
    let log_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_file_path)
        .with_context(|| format!("Failed to open log file: {}", log_file_path))?;
    let level_filter = if only_parse {
        LevelFilter::INFO
    } else {
        LevelFilter::from_level(log_level)
    };
    Registry::default()
        .with(level_filter)
        .with(
            tracing_subscriber::fmt::Layer::default()
                .with_writer(std::sync::Mutex::new(log_file))
                .with_ansi(false)
                .with_timer(tracing_subscriber::fmt::time::ChronoUtc::rfc_3339()),
        )
        .with(
            tracing_subscriber::fmt::Layer::default()
                .with_writer(std::io::stdout)
                .with_timer(tracing_subscriber::fmt::time::ChronoUtc::rfc_3339()),
        )
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
    cache_only: bool,
) -> anyhow::Result<(Option<Arc<AuthClient>>, HashSet<String>)> {
    let cache_data = read_cached_ids();
    info!(
        "Cache: {} existing action IDs from {} resource(s)",
        format_number(cache_data.action_ids.len()),
        cache_data.fetched_resources.len()
    );

    let auth_client = Arc::new(AuthClient::new(config.clone()));
    let _token = auth_client
        .get_valid_token()
        .await
        .context("Failed to authenticate with Halo API")?;
    info!("Authentication successful");

    if cache_only {
        info!(
            "Cache mode: using {} cached IDs, skipping report fetching",
            format_number(cache_data.action_ids.len())
        );
        if only_parse {
            return Ok((None, cache_data.action_ids));
        }
        return Ok((Some(auth_client), cache_data.action_ids));
    }

    let report_client = ReportClient::new(config.clone(), auth_client.clone());
    let report_ids = report_client
        .get_existing_action_ids(&cache_data.fetched_resources)
        .await
        .context("Failed to fetch existing action IDs from report")?;
    info!(
        "Fetched {} action IDs from reports",
        format_number(report_ids.len())
    );

    let mut all_ids = cache_data.action_ids;
    let before_merge = all_ids.len();
    all_ids.extend(report_ids);
    let new_from_reports = all_ids.len() - before_merge;

    info!(
        "Total {} existing action IDs to skip ({} new from reports)",
        format_number(all_ids.len()),
        format_number(new_from_reports)
    );

    if only_parse {
        info!(
            "Parse-only mode: existing IDs fetched successfully, will skip API calls for imports"
        );
        return Ok((None, all_ids));
    }
    Ok((Some(auth_client), all_ids))
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
    cache_only: bool,
    input_path: &str,
) -> anyhow::Result<SetupResult> {
    // Fail fast if no files before expensive ID fetching
    let files_to_process = discover_files(input_path)?;
    if files_to_process.is_empty() {
        anyhow::bail!(
            "No CSV or Excel files found in input directory: {}. Nothing to process.",
            input_path
        );
    }

    let (auth_client, existing_ids) =
        setup_auth_and_existing_ids(config, only_parse, cache_only).await?;
    let action_client = auth_client
        .as_ref()
        .map(|auth| ActionClient::new(config.clone(), auth.clone()));

    Ok(SetupResult {
        existing_ids,
        action_client,
        files_to_process,
        auth_client,
    })
}
