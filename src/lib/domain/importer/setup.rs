use std::collections::{BTreeSet, HashSet};
use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use chrono::Utc;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use tracing_subscriber::Registry;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use super::format::format_number;
use crate::config::Config;
use crate::domain::models::action_object::ActionId;
use crate::inbound::client::ReportClient;
use crate::outbound::client::action::ActionClient;
use crate::outbound::client::auth::AuthClient;

const LOG_DIR: &str = "log";
const CACHE_DIR: &str = "cache";
const RESOURCE_CACHE_FILE_NAME: &str = "existing.json";
const RESOURCE_CACHE_LOCK_FILE_NAME: &str = "existing.json.lock";
const IMPORTED_CACHE_FILE_NAME: &str = "imported";

#[derive(Debug, Serialize, Deserialize)]
struct ResourceCache {
    resource_id: String,
    action_ids: Vec<String>,
}

pub struct CacheData {
    pub action_ids: HashSet<ActionId>,
    pub fetched_resources: HashSet<String>,
}

/// Parses ids read from `source` as numbers. An id that is not a number can
/// never match an action, so it is left out and counted in one warning.
pub fn parse_action_ids<'a>(ids: impl IntoIterator<Item = &'a str>, source: &str) -> Vec<ActionId> {
    let mut parsed = Vec::new();
    let mut rejected = 0usize;
    for id in ids {
        if id.trim().is_empty() {
            continue;
        }
        match id.parse::<ActionId>() {
            Ok(id) => parsed.push(id),
            Err(_) => rejected += 1,
        }
    }
    if rejected > 0 {
        warn!(
            "Ignored {} action ID(s) in {} that are not whole numbers",
            format_number(rejected),
            source
        );
    }
    parsed
}

/// The cache directory: `existing.json`, the ids each report returned, and
/// `imported`, the ids this importer posted, one per line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cache {
    dir: PathBuf,
}

impl Default for Cache {
    /// The `cache` directory under the working directory.
    fn default() -> Self {
        Self::new(CACHE_DIR)
    }
}

impl Cache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Reads cached action IDs from both existing (JSON format) and imported (line-separated) files.
    /// A resource cache that exists but cannot be read or parsed is an error naming the file.
    pub fn read(&self) -> anyhow::Result<CacheData> {
        let mut action_ids = HashSet::new();
        let mut fetched_resources = HashSet::new();

        let resource_path = self.dir.join(RESOURCE_CACHE_FILE_NAME);
        let resource_source = resource_path.display().to_string();
        for resource in read_resource_cache(&resource_path)? {
            action_ids.extend(parse_action_ids(
                resource.action_ids.iter().map(String::as_str),
                &resource_source,
            ));
            fetched_resources.insert(resource.resource_id);
        }

        // Read imported IDs (simple text file, one ID per line)
        let txt_path = self.dir.join(IMPORTED_CACHE_FILE_NAME);
        if txt_path.exists()
            && let Ok(file) = std::fs::File::open(&txt_path)
        {
            let reader = std::io::BufReader::new(file);
            let lines: Vec<String> = std::io::BufRead::lines(reader)
                .map_while(Result::ok)
                .collect();
            action_ids.extend(parse_action_ids(
                lines.iter().map(String::as_str),
                &txt_path.display().to_string(),
            ));
        }

        Ok(CacheData {
            action_ids,
            fetched_resources,
        })
    }

    /// Merges a report's action IDs into the existing cache file (JSON format with resource tracking).
    pub fn merge_report(&self, resource_id: &str, action_ids: &[ActionId]) -> anyhow::Result<()> {
        if action_ids.is_empty() {
            return Ok(());
        }
        let cache_dir = &self.dir;

        std::fs::create_dir_all(cache_dir).with_context(|| {
            format!("Failed to create cache directory: {}", cache_dir.display())
        })?;
        // Writers take a separate lock file, since the cache file itself is
        // replaced by a rename on every write.
        let lock_path = cache_dir.join(RESOURCE_CACHE_LOCK_FILE_NAME);
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("Failed to open cache lock file: {}", lock_path.display()))?;
        lock.lock_exclusive()
            .with_context(|| "Failed to acquire exclusive lock on cache file")?;

        let cache_path = cache_dir.join(RESOURCE_CACHE_FILE_NAME);
        let mut resources = read_resource_cache(&cache_path)?;

        // Merge with the resource's cached IDs as numbers, so `0042` and `42` are one ID.
        // A cached entry that is not a number is kept as it was.
        let position = resources.iter().position(|r| r.resource_id == resource_id);
        let existing = position
            .and_then(|index| resources.get(index))
            .map(|r| r.action_ids.clone())
            .unwrap_or_default();
        let mut numeric: BTreeSet<ActionId> = action_ids.iter().copied().collect();
        let mut other: Vec<String> = Vec::new();
        for id in existing {
            match id.parse::<ActionId>() {
                Ok(parsed) => {
                    numeric.insert(parsed);
                }
                Err(_) => other.push(id),
            }
        }
        let mut merged: Vec<String> = numeric.iter().map(ActionId::to_string).collect();
        merged.extend(other);
        match position.and_then(|index| resources.get_mut(index)) {
            Some(resource) => resource.action_ids = merged,
            None => resources.push(ResourceCache {
                resource_id: resource_id.to_string(),
                action_ids: merged,
            }),
        }

        // Serialize to JSON with pretty formatting
        let json = serde_json::to_string_pretty(&resources)
            .with_context(|| "Failed to serialize cache to JSON")?;

        write_atomically(&cache_path, json.as_bytes())
    }

    /// Appends imported action IDs to the imported file, under a lock for concurrent writers.
    pub fn record_imported(&self, ids: &[String]) -> anyhow::Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let path = self.dir.join(IMPORTED_CACHE_FILE_NAME);

        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("Failed to create cache directory: {}", self.dir.display()))?;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("Failed to open imported cache file: {}", path.display()))?;

        // Acquire exclusive lock to prevent concurrent writes from other processes
        file.lock_exclusive()
            .with_context(|| "Failed to acquire exclusive lock on imported cache file")?;

        for id in ids {
            writeln!(file, "{}", id)?;
        }

        // Lock is automatically released when file is dropped
        Ok(())
    }
}

/// Reads the resource cache at `path`: nothing when the file is missing or
/// blank, an error naming the file when it cannot be read or is not valid.
fn read_resource_cache(path: &Path) -> anyhow::Result<Vec<ResourceCache>> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("Failed to read cache file: {}", path.display()));
        }
    };
    if contents.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&contents).with_context(|| {
        format!(
            "Cache file {} is not valid; fix or delete it and rerun",
            path.display()
        )
    })
}

/// Writes `contents` to a temporary file next to `path` and renames it over
/// `path`, so a reader sees the old file or the new one, never half of one.
fn write_atomically(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    let file_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .with_context(|| format!("Cache path has no file name: {}", path.display()))?;
    let temp_path = path.with_file_name(format!("{file_name}.tmp-{}", std::process::id()));
    let written = std::fs::File::create(&temp_path)
        .and_then(|mut file| {
            file.write_all(contents)?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&temp_path, path));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&temp_path);
        return Err(e).with_context(|| format!("Failed to write cache file: {}", path.display()));
    }
    Ok(())
}

/// The two command line switches setup reads: parse without posting, and
/// trust the ID cache instead of fetching the reports. Both can be on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SetupOptions {
    pub only_parse: bool,
    pub cache_only: bool,
}

pub struct SetupResult {
    pub existing_ids: HashSet<ActionId>,
    pub action_client: Option<ActionClient>,
    pub files_to_process: Vec<(PathBuf, String)>,
    pub auth_client: Option<Arc<AuthClient>>,
}

pub fn setup_logging(only_parse: bool, log_level: tracing::Level) -> anyhow::Result<String> {
    std::fs::create_dir_all(LOG_DIR)
        .with_context(|| format!("Failed to create log directory: {}", LOG_DIR))?;

    let timestamp_str = Utc::now().format("%Y-%m-%d_%H-%M-%S");
    let log_dir_path = format!("{}/{}", LOG_DIR, timestamp_str);

    // Create the timestamped directory
    std::fs::create_dir_all(&log_dir_path).with_context(|| {
        format!(
            "Failed to create timestamped log directory: {}",
            log_dir_path
        )
    })?;

    // Create full.log inside the directory
    let log_file_path = format!("{}/full.log", log_dir_path);
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
    Ok(log_dir_path)
}

pub async fn setup_auth_and_existing_ids(
    config: &Config,
    options: SetupOptions,
    cache: &Cache,
) -> anyhow::Result<(Option<Arc<AuthClient>>, HashSet<ActionId>)> {
    let cache_data = cache.read()?;
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

    if options.cache_only {
        info!(
            "Cache mode: using {} cached IDs, skipping report fetching",
            format_number(cache_data.action_ids.len())
        );
        if options.only_parse {
            return Ok((None, cache_data.action_ids));
        }
        return Ok((Some(auth_client), cache_data.action_ids));
    }

    let report_client = ReportClient::new(config.clone(), auth_client.clone());
    let report_ids = report_client
        .get_existing_action_ids(cache, &cache_data.fetched_resources)
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

    if options.only_parse {
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
    options: SetupOptions,
    cache: &Cache,
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

    let (auth_client, existing_ids) = setup_auth_and_existing_ids(config, options, cache).await?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_takes_spreadsheets_at_the_top_level_only_whatever_the_case() {
        let dir = std::env::temp_dir().join(format!("halo_discover_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        for name in ["a.csv", "b.XLSX", "c.xls", "notes.txt", "nested/d.csv"] {
            std::fs::write(dir.join(name), "").unwrap();
        }

        let mut names: Vec<String> = discover_files(dir.to_str().unwrap())
            .unwrap()
            .into_iter()
            .map(|(_, name)| name)
            .collect();
        names.sort();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(names, ["a.csv", "b.XLSX", "c.xls"]);
    }

    #[test]
    fn a_missing_input_directory_is_named() {
        let message = discover_files("/nonexistent/halo").unwrap_err().to_string();
        assert!(message.contains("/nonexistent/halo"), "{message}");
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("halo_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn cached_ids_compare_as_numbers() {
        let dir = scratch_dir("cache_numbers");
        std::fs::write(
            dir.join(RESOURCE_CACHE_FILE_NAME),
            r#"[{"resource_id":"r","action_ids":["0042"," 7 ","abc"]}]"#,
        )
        .unwrap();
        std::fs::write(dir.join(IMPORTED_CACHE_FILE_NAME), "0100\n\nxyz\n").unwrap();

        let cache = Cache::new(&dir).read().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let expected: HashSet<ActionId> = [42, 7, 100].into_iter().map(ActionId::new).collect();
        assert_eq!(cache.action_ids, expected);
        assert!(cache.fetched_resources.contains("r"));
    }

    #[test]
    fn appending_to_the_resource_cache_merges_ids_as_numbers() {
        let dir = scratch_dir("cache_merge");
        std::fs::write(
            dir.join(RESOURCE_CACHE_FILE_NAME),
            r#"[{"resource_id":"r","action_ids":["0042","9"]}]"#,
        )
        .unwrap();

        Cache::new(&dir)
            .merge_report("r", &[ActionId::new(42), ActionId::new(10)])
            .unwrap();

        let written = std::fs::read_to_string(dir.join(RESOURCE_CACHE_FILE_NAME)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let resources: Vec<ResourceCache> = serde_json::from_str(&written).unwrap();
        assert_eq!(resources[0].action_ids, ["9", "10", "42"]);
    }

    #[test]
    fn appending_to_a_corrupt_resource_cache_names_the_file_and_leaves_it_alone() {
        let dir = scratch_dir("cache_corrupt_append");
        let path = dir.join(RESOURCE_CACHE_FILE_NAME);
        std::fs::write(&path, "[{ not json").unwrap();

        let result = Cache::new(&dir).merge_report("r", &[ActionId::new(1)]);

        let contents = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("existing.json"), "{message}");
        assert_eq!(contents, "[{ not json");
    }

    #[cfg(unix)]
    #[test]
    fn the_resource_cache_is_replaced_whole_rather_than_rewritten_in_place() {
        use std::os::unix::fs::MetadataExt;

        let dir = scratch_dir("cache_rename");
        let path = dir.join(RESOURCE_CACHE_FILE_NAME);
        std::fs::write(&path, "[]").unwrap();
        let before = std::fs::metadata(&path).unwrap().ino();

        Cache::new(&dir)
            .merge_report("r", &[ActionId::new(1)])
            .unwrap();

        let after = std::fs::metadata(&path).unwrap().ino();
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_ne!(before, after);
        assert!(names.iter().all(|name| !name.contains(".tmp")), "{names:?}");
    }

    #[test]
    fn a_corrupt_resource_cache_is_an_error_naming_the_file() {
        let dir = scratch_dir("cache_corrupt_read");
        std::fs::write(dir.join(RESOURCE_CACHE_FILE_NAME), "[{ not json").unwrap();

        let result = Cache::new(&dir).read();
        std::fs::remove_dir_all(&dir).unwrap();
        let message = format!("{:#}", result.err().unwrap());
        assert!(message.contains("existing.json"), "{message}");
    }
}
