# Processing Flow

## High-Level Flow

```
┌────────────────────────────────────────────────────────────┐
│ 1. CLI Entry (main.rs)                                     │
│    • Parse arguments (batch-size, only-parse, input-path)  │
│    • Load .env configuration                               │
└────────────────┬───────────────────────────────────────────┘
                 │
┌────────────────▼───────────────────────────────────────────┐
│ 2. Setup Phase (setup.rs)                                  │
│    • Initialize logging (file + console)                   │
│    • Load cached IDs from disk                             │
│    • Authenticate with Halo OAuth2                         │
│    • Fetch existing IDs from reports                       │
│    • Discover input files                                  │
└────────────────┬───────────────────────────────────────────┘
                 │
┌────────────────▼───────────────────────────────────────────┐
│ 3. File Processing Loop (processor.rs)                     │
│    For each CSV/Excel file:                                │
│    ├─→ Create iterator (streaming CSV or in-memory Excel)  │
│    ├─→ Process rows → batch → POST to API                  │
│    └─→ Collect ProcessingStats                             │
└────────────────┬───────────────────────────────────────────┘
                 │
┌────────────────▼───────────────────────────────────────────┐
│ 4. Summary Phase (summary.rs)                              │
│    • Aggregate statistics from all files                   │
│    • Calculate performance metrics                         │
│    • Log final summary                                     │
└────────────────────────────────────────────────────────────┘
```

---

## Detailed Breakdown

### 1. CLI Entry (bin/main.rs)

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse CLI arguments
    let args: Vec<String> = std::env::args().collect();
    let mut batch_size = 1;
    let mut only_parse = false;
    let mut input_path = "input";
    let mut only_use_cache = false;

    // Parse flags: --batch-size, --only-parse, --input-path, --only-use-cache
    // ...

    // Load configuration from .env
    let config = Config::from_env()?;

    // Initialize logging
    setup_logging(only_parse, config.log_level)?;
    info!("Starting Halo action importer");

    // Setup phase
    let (auth_client, action_client, files, existing_ids) =
        setup(&config, input_path, only_parse, only_use_cache).await?;

    // Process files
    let all_stats = process_all_files(
        &files,
        &existing_ids,
        &action_client,
        &auth_client,
        batch_size,
        only_parse
    ).await;

    // Log summary
    log_summary(all_stats, start_time);

    Ok(())
}
```

**CLI Arguments**:
| Argument | Aliases | Effect |
|----------|---------|--------|
| `--batch-size N` | `--bs N`, `--batch N` | Post N actions per API call |
| `--only-parse-inputs` | `--op` | Validate files without API calls |
| `--input-path PATH` | `--ip PATH` | Use custom input directory |
| `--only-use-cache` | `--oc` | Skip report fetching, use local cache |

---

### 2. Setup Phase (domain/importer/setup.rs)

#### 2.1 Setup Logging
```rust
pub fn setup_logging(only_parse: bool, log_level: Level) -> anyhow::Result<()> {
    // Create log directory
    std::fs::create_dir_all("log")?;

    // Generate timestamped filename
    let timestamp = Utc::now().format("%Y-%m-%d_%H-%M-%S");
    let log_file_path = format!("log/{}.log", timestamp);

    // Open log file
    let log_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_file_path)?;

    // Initialize dual output (file + console)
    Registry::default()
        .with(level_filter)
        .with(fmt::Layer::default()
            .with_writer(Mutex::new(log_file))
            .with_ansi(false)
            .with_timer(ChronoUtc::rfc_3339()))
        .with(fmt::Layer::default()
            .with_writer(std::io::stdout)
            .with_timer(ChronoUtc::rfc_3339()))
        .init();
}
```

#### 2.2 Load Cached IDs
```rust
pub fn read_cached_ids() -> anyhow::Result<CacheData> {
    let mut action_ids = HashSet::new();
    let mut fetched_resources = HashSet::new();

    // Read cache/existing_action_ids.json
    if Path::new("cache/existing_action_ids.json").exists() {
        let file = File::open("cache/existing_action_ids.json")?;
        let caches: Vec<ResourceCache> = serde_json::from_reader(file)?;

        for cache in caches {
            fetched_resources.insert(cache.resource_id);
            action_ids.extend(cache.action_ids);
        }
    }

    // Read cache/imported_ids.txt
    if Path::new("cache/imported_ids.txt").exists() {
        let content = std::fs::read_to_string("cache/imported_ids.txt")?;
        for line in content.lines() {
            action_ids.insert(line.trim().to_string());
        }
    }

    info!("Cache: {} existing action IDs from {} resource(s)",
          action_ids.len(), fetched_resources.len());

    Ok(CacheData { action_ids, fetched_resources })
}
```

#### 2.3 Authenticate
```rust
// Create AuthClient
let auth_client = AuthClient::new(
    config.token_url.clone(),
    config.client_id.clone(),
    config.client_secret.clone(),
);

// Fetch initial token
let token = auth_client.get_valid_token().await?;
info!("Authentication successful");
```

#### 2.4 Fetch Existing IDs from Reports
```rust
pub async fn fetch_and_cache_action_ids(
    config: &Config,
    auth_client: &AuthClient,
    existing_ids: &mut HashSet<String>,
    fetched_resources: &HashSet<String>,
    only_use_cache: bool,
) -> anyhow::Result<usize> {
    if only_use_cache {
        info!("Using only cached IDs (--only-use-cache flag)");
        return Ok(0);
    }

    let report_client = ReportClient::new(
        config.base_resource_url.clone(),
        auth_client,
    );

    let mut new_ids_count = 0;

    for uuid in &config.action_ids_resource_uuids {
        // Skip if already fetched
        if fetched_resources.contains(uuid) {
            continue;
        }

        // Fetch IDs from report
        let ids = report_client.fetch_action_ids(uuid).await?;
        info!("Fetched {} action IDs from report {}", ids.len(), uuid);

        // Add to existing set
        new_ids_count += ids.len();
        existing_ids.extend(ids.clone());

        // Update cache on disk
        append_resource_to_cache(uuid, &ids)?;
    }

    info!("Total {} existing action IDs to skip ({} new from reports)",
          existing_ids.len(), new_ids_count);

    Ok(new_ids_count)
}
```

#### 2.5 Discover Files
```rust
pub fn discover_files(input_path: &str) -> anyhow::Result<Vec<(PathBuf, String)>> {
    let mut files = Vec::new();

    for entry in std::fs::read_dir(input_path)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() {
            let extension = path.extension()
                .and_then(|s| s.to_str())
                .unwrap_or("");

            match extension {
                "csv" | "xlsx" | "xls" => {
                    let filename = path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .to_string();
                    files.push((path, filename));
                }
                _ => {}
            }
        }
    }

    info!("Starting import of {} file(s)", files.len());
    Ok(files)
}
```

---

### 3. File Processing Loop (domain/importer/processor.rs)

#### 3.1 CSV Processing
```rust
pub async fn process_csv_file(
    file_path: &Path,
    file_name: &str,
    existing_ids: &HashSet<String>,
    action_client: &ActionClient,
    auth_client: &AuthClient,
    batch_size: usize,
    only_parse: bool,
    sheet_index: usize,
    total_sheets: usize,
) -> anyhow::Result<ProcessingStats> {
    // Create streaming iterator
    let (total_rows, mut iter) = csv_action_iter(file_path)?;

    info!("Processing sheet {} of {}: CSV file '{}' ({} rows)",
          sheet_index, total_sheets, file_name, total_rows);

    // Initialize tracking
    let mut stats = ProcessingStats::default();
    let mut batch: Vec<ActionObject> = Vec::with_capacity(batch_size);
    let mut missing_tickets: HashSet<u32> = HashSet::new();
    let mut pending_skips = 0;

    // Progress tracking
    let start_time = Instant::now();
    let mut last_log_time = start_time;
    let mut row_times: Vec<f64> = Vec::new();

    // Process each row
    while let Some(action_result) = iter.next() {
        stats.processed += 1;

        // Deserialize ActionObject
        let action = match action_result {
            Ok(a) => a,
            Err(e) => {
                error!("Failed to deserialize row in CSV file '{}': {}", file_name, e);
                stats.failed.push(("unknown".to_string(), e.to_string()));
                continue;
            }
        };

        // Check if action already exists
        if existing_ids.contains(action.action_id.value()) {
            stats.skipped += 1;
            pending_skips += 1;
            continue;
        }

        // Check if ticket is missing
        if missing_tickets.contains(&action.ticket_id) {
            stats.skipped += 1;
            pending_skips += 1;
            continue;
        }

        // Add to batch
        batch.push(action);

        // Flush batch if full
        if batch.len() >= batch_size {
            flush_batch(
                &mut batch,
                &mut pending_skips,
                &mut stats,
                &mut missing_tickets,
                &mut row_times,
                action_client,
                only_parse,
            ).await?;
        }

        // Log progress periodically
        log_progress_if_needed(
            &stats,
            total_rows,
            &mut last_log_time,
            &row_times,
            only_parse,
            sheet_index,
            total_sheets,
            file_name,
            "CSV",
        );
    }

    // Flush remaining batch
    flush_batch(/* ... */).await?;

    // Log final skip count
    if pending_skips > 0 {
        info!("Skipped {} entries (already exist)", pending_skips);
    }

    Ok(stats)
}
```

#### 3.2 Excel Processing
```rust
pub async fn process_excel_file(
    file_path: &Path,
    file_name: &str,
    /* ... same parameters as CSV ... */
) -> anyhow::Result<ProcessingStats> {
    // Create in-memory iterator
    let (sheet_name, total_rows, mut iter) = excel_action_iter(file_path)?;

    info!("Processing sheet {} of {}: Excel file '{}', sheet '{}' ({} rows)",
          sheet_index, total_sheets, file_name, sheet_name, total_rows);

    // Process rows (same logic as CSV)
    // ...
}
```

**Key Difference**: Excel loads all rows into memory (calamine limitation), while CSV streams row-by-row.

#### 3.3 Batch Flushing
```rust
async fn flush_batch(
    batch: &mut Vec<ActionObject>,
    pending_skips: &mut usize,
    stats: &mut ProcessingStats,
    missing_tickets: &mut HashSet<u32>,
    row_times: &mut Vec<f64>,
    action_client: &ActionClient,
    only_parse: bool,
) -> anyhow::Result<()> {
    if batch.is_empty() {
        return Ok(());
    }

    // Log pending skips
    if *pending_skips > 0 {
        info!("Skipped {} entries (already exist)", pending_skips);
        *pending_skips = 0;
    }

    if only_parse {
        // Parse-only mode: just count
        stats.imported += batch.len();
        batch.clear();
        return Ok(());
    }

    // Record start time
    let batch_start = Instant::now();

    // POST to API
    match action_client.post_action_objects(batch.clone()).await {
        Ok(_) => {
            // Success
            let duration = batch_start.elapsed().as_secs_f64();
            row_times.push(duration / batch.len() as f64);

            // Log success message
            if batch.len() == 1 {
                let action = &batch[0];
                info!("Success: imported action ID: {} (ticket ID: {})",
                      action.action_id.value(), action.ticket_id);
            } else {
                let action_ids: Vec<&str> = batch.iter()
                    .map(|a| a.action_id.value())
                    .collect();
                let ticket_ids: HashSet<u32> = batch.iter()
                    .map(|a| a.ticket_id)
                    .collect();
                info!("Success: imported batch of {} actions | action IDs: {} | ticket IDs: {}",
                      batch.len(),
                      action_ids.join(", "),
                      ticket_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(", "));
            }

            // Update cache
            let imported_ids: Vec<String> = batch.iter()
                .map(|a| a.action_id.value().to_string())
                .collect();
            if let Err(e) = append_imported_ids_to_cache(&imported_ids) {
                warn!("Failed to update cache with imported IDs: {}", e);
            }

            stats.imported += batch.len();
        }
        Err(e) => {
            // Failure - UPDATED 2026-01-27: Now uses ticket-grouped retry
            let error_str = e.to_string();

            // If "not found" error and batch > 1, use ticket-grouped retry
            if (error_str.contains("not found") || error_str.contains("404")) && batch.len() > 1 {
                warn!("Batch failed with 'not found' error - using ticket-grouped retry");

                // Call retry_by_ticket_groups() - groups actions by ticket_id
                // Retries each ticket group independently
                // Successfully imports actions for valid tickets
                // Marks all actions for missing tickets as failed
                let result = retry_by_ticket_groups(action_client, batch).await;

                // Update stats with recovered actions
                stats.imported += result.imported_actions.len();
                stats.failed.extend(result.failed_actions);

                // Add missing tickets to skip set
                for ticket_id in result.missing_tickets {
                    missing_tickets.insert(ticket_id);
                    warn!("Ticket ID: {} not found - will skip future actions for this ticket", ticket_id);
                }

                info!("Ticket group retry complete: recovered {}/{} actions, identified {} missing ticket(s)",
                      result.imported_actions.len(),
                      batch.len(),
                      result.missing_tickets.len());
            } else {
                // Not a "not found" error, or batch size is 1
                // Check for missing ticket
                if error_str.contains("not found") || error_str.contains("404") {
                    let ticket_id = batch[0].ticket_id;
                    missing_tickets.insert(ticket_id);
                    warn!("Ticket ID: {} not found - will skip future actions for this ticket", ticket_id);
                }

                // Log error for each action
                for action in batch.iter() {
                    let error_msg = format!(
                        "Failed to import action ID: {} (ticket ID: {}): {}",
                        action.action_id.value(),
                    action.ticket_id,
                    e
                );
                error!("{}", error_msg);
                stats.failed.push((action.action_id.value().to_string(), error_msg));
            }
        }
    }

    batch.clear();
    Ok(())
}
```

#### 3.4 Progress Logging
```rust
fn log_progress_if_needed(
    stats: &ProcessingStats,
    total_rows: usize,
    last_log_time: &mut Instant,
    row_times: &[f64],
    only_parse: bool,
    sheet_index: usize,
    total_sheets: usize,
    file_name: &str,
    file_type: &str,
) {
    let now = Instant::now();
    let elapsed = now.duration_since(*last_log_time).as_secs();

    // Frequency: 100 entries or 60s (import), 10000 entries or 5s (parse-only)
    let should_log = if only_parse {
        stats.processed % 10000 == 0 || elapsed >= 5
    } else {
        stats.processed % 100 == 0 || elapsed >= 60
    };

    if !should_log {
        return;
    }

    *last_log_time = now;

    // Calculate average time per row (only counts imports)
    let avg_time_per_row = if !row_times.is_empty() {
        row_times.iter().sum::<f64>() / row_times.len() as f64
    } else {
        0.0
    };

    // Calculate estimated time remaining
    let remaining_rows = total_rows - stats.processed;
    let est_remaining_secs = remaining_rows as f64 * avg_time_per_row;
    let est_remaining_str = format_duration(est_remaining_secs);

    // Calculate percentage
    let percentage = (stats.processed as f64 / total_rows as f64) * 100.0;

    info!("Progress [sheet {} of {}: '{}' - {} '{}']: {}/{} rows ({:.1}%), {} imported, {} skipped | avg {:.2}s/row | est. remaining: {}",
          sheet_index,
          total_sheets,
          file_name,
          file_type,
          file_name,
          stats.processed,
          total_rows,
          percentage,
          stats.imported,
          stats.skipped,
          avg_time_per_row,
          est_remaining_str);
}
```

---

### 4. Summary Phase (domain/importer/summary.rs)

```rust
pub fn log_summary(
    all_stats: Vec<ProcessingStats>,
    skipped_files: Vec<String>,
    start_time: Instant,
) {
    // Aggregate statistics
    let mut summary = ImportSummary::default();

    for stats in all_stats {
        summary.total_processed += stats.processed;
        summary.total_imported += stats.imported;
        summary.total_skipped += stats.skipped;
        summary.total_failed += stats.failed.len();
    }

    summary.skipped_files = skipped_files;
    summary.total_runtime_secs = start_time.elapsed().as_secs_f64();

    // Calculate metrics
    let time_per_entry = if summary.total_imported > 0 {
        summary.total_runtime_secs / summary.total_imported as f64
    } else {
        0.0
    };

    let entries_per_minute = if summary.total_runtime_secs > 0.0 {
        (summary.total_imported as f64 / summary.total_runtime_secs) * 60.0
    } else {
        0.0
    };

    // Log summary
    info!("=== Import Summary ===");
    info!("Total actions processed: {}", format_number(summary.total_processed));
    info!("Actions skipped (already exist): {}", format_number(summary.total_skipped));
    info!("Actions successfully imported: {}", format_number(summary.total_imported));
    info!("Actions failed to import: {}", format_number(summary.total_failed));

    if !summary.skipped_files.is_empty() {
        warn!("Files that could not be read: {}", summary.skipped_files.len());
    }

    info!("=== Performance Stats ===");
    info!("Total runtime: {:.2}s ({:.2}m)",
          summary.total_runtime_secs,
          summary.total_runtime_secs / 60.0);
    info!("Time per entry: {:.3}s", time_per_entry);
    info!("Entries per minute: {:.1}", entries_per_minute);

    if !summary.sheet_times.is_empty() {
        let avg_sheet_time = summary.sheet_times.iter().sum::<f64>()
                           / summary.sheet_times.len() as f64;
        info!("Average time per sheet: {:.2}s", avg_sheet_time);
    }
}
```

---

## Key Data Flow Points

### 1. Existing IDs (Duplicate Detection)
```
Reports (via API) → HashSet<String> ← Cache JSON file
                         ↓
              Lookup during processing (O(1))
                         ↓
              Skip if action_id exists
```

### 2. Missing Tickets (Optimization)
```
First API error "not found" → HashSet<u32>
                                    ↓
              Lookup before API call (O(1))
                                    ↓
              Skip without API call if ticket_id exists
```

### 3. Failed Actions (Error Tracking)
```
API error → Vec<(action_id, error_message)>
                         ↓
              Aggregated in ImportSummary
                         ↓
              Count logged in final summary
```

### 4. Successful Imports (Cache Update)
```
API success → action_ids extracted
                   ↓
        append_imported_ids_to_cache()
                   ↓
        cache/imported_ids.txt (append mode)
                   ↓
        Loaded on next run to skip re-imports
```

---

## Performance Characteristics

### CSV Processing
- **Memory**: ~O(1) - streaming iterator
- **Throughput**: Limited by API calls (500ms delay per batch)
- **File Size**: Can handle multi-GB files

### Excel Processing
- **Memory**: O(n) - all rows loaded into memory
- **Throughput**: Same as CSV (API-bound)
- **File Size**: Limited by available RAM

### Batch Processing
- **batch_size = 1**: ~1000-1500 entries/min (default)
- **batch_size = 10**: ~5000-8000 entries/min (10x throughput)
- **batch_size = 100**: ~20000-30000 entries/min (limited by API)

### Parallel Execution
- **Independent instances**: Linear scaling (3 instances = 3x throughput)
- **Shared cache**: Each instance updates its own cache file
- **No conflicts**: Each instance processes different files
