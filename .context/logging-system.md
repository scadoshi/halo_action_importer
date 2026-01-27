# Logging System

## Overview
Dual-output logging system using `tracing` crate with structured timestamps (RFC 3339 format).

## Implementation Location
**File**: `src/lib/domain/importer/setup.rs` (lines 171-207)

## Initialization

```rust
pub fn setup_logging(only_parse: bool, log_level: tracing::Level) -> anyhow::Result<()> {
    std::fs::create_dir_all(LOG_DIR)?;  // LOG_DIR = "log"
    let timestamp_str = Utc::now().format("%Y-%m-%d_%H-%M-%S");
    let log_file_path = format!("{}/{}.log", LOG_DIR, timestamp_str);

    let log_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_file_path)?;

    Registry::default()
        .with(level_filter)
        .with(tracing_subscriber::fmt::Layer::default()
            .with_writer(std::sync::Mutex::new(log_file))
            .with_ansi(false)  // No color codes in file
            .with_timer(tracing_subscriber::fmt::time::ChronoUtc::rfc_3339()))
        .with(tracing_subscriber::fmt::Layer::default()
            .with_writer(std::io::stdout)
            .with_timer(tracing_subscriber::fmt::time::ChronoUtc::rfc_3339()))
        .init();
}
```

## Current Log Structure

### Directory-Based Output (Updated 2026-01-27)
- **Location**: `log/YYYY-MM-DD_HH-MM-SS/` (directory, not single file)
- **Format**: RFC 3339 timestamps + message
- **Example**: `log/2026-01-27_05-35-26/`

### Files Generated Per Run
1. **full.log**: Complete log of all messages
2. **retry.csv**: Failed actions that need retry (if any failures)
3. **summary.json**: Machine-readable statistics

### Dual Output Targets
1. **File**: No ANSI color codes, permanent record
2. **Console (stdout)**: With ANSI colors (if terminal supports), real-time monitoring

## Log Levels

| Level | Usage | Example |
|-------|-------|---------|
| `TRACE` | Most verbose, all internal operations | Rarely used |
| `DEBUG` | Debug info, detailed flow | HTTP request/response details |
| `INFO` | General info, progress, summaries | **Default** - most application logs |
| `WARN` | Warnings, non-critical errors | Missing tickets, cache write failures |
| `ERROR` | Errors that prevent operations | API failures, deserialization errors |

**Configuration**: Via `LOG_LEVEL` env var in `.env` file
**Parse-Only Mode**: Forces `INFO` level regardless of config

## Log Message Types

### 1. Startup Logs (INFO)
```
INFO Starting Halo action importer
INFO Configuration loaded successfully
INFO Cache: 1,034,479 existing action IDs from 3 resource(s)
INFO Authentication successful
INFO Fetched 500,000 action IDs from reports
INFO Total 1,534,479 existing action IDs to skip (500,000 new from reports)
INFO Starting import of 9 file(s)
```

### 2. File Processing Start (INFO)
```
INFO Processing sheet 1 of 9: Excel file 'IncidentJournals_LastModPriorToJune2025_7.xlsx',
     sheet 'IncidentJournals_ClosedIncs_Las' (350,532 rows)
```

### 3. Progress Updates - REMOVED (2026-01-27)
**Previous Behavior** (no longer implemented):
- Periodic updates every 100 entries OR 60 seconds
- Created noise in logs, made them harder to read

**Current Behavior**:
- No periodic progress updates
- Progress shown only through success logs (see section 5 below)
- Each successful import shows current position and ETA

### 4. Skip Messages (INFO - Batched)
**Purpose**: Reduce log clutter when many consecutive actions are skipped
**Format**:
```
INFO Skipped 1,234 entries (already exist)
```

**Implementation**: Counter accumulates skips, flushes when:
- A non-skip action occurs
- Batch is posted
- File processing completes

### 5. Success Messages (INFO) - Enhanced (2026-01-27)

**Batch Size = 1** (default):
```
INFO Imported action 1/10000 (ID: 12345, ticket: 67890) | 50 total skipped | 0.50s/row | ETA: 1h 23m 15s
```

**Components**:
- Current action / total actions
- Action ID and ticket ID
- Cumulative skip count
- Average time per row
- Estimated time remaining

**Batch Size > 1**:
```
INFO Imported batch 1/200 (50 actions, tickets: 67890, 67891, 67892, 67893) | 50 total skipped | 0.05s/action | ETA: 8m 15s
```

**Components**:
- Current batch / total batches (using explicit counter, not calculated)
- Number of actions in batch
- **All ticket IDs** (no truncation, all tickets shown comma-separated)
- Cumulative skip count
- Average time per action
- Estimated time remaining

**Note**:
- Ticket IDs are deduplicated (multiple actions can share same ticket)
- Batch numbers use explicit counter (incremented on success) to avoid off-by-one errors

### 6. Warning Messages (WARN)

**Ticket-Grouped Retry** (when batch fails with "not found" error):
```
WARN Batch failed with 'not found' error - retrying 3 ticket groups
WARN Ticket ID: 438843 not found - marking 7 action(s) as failed
INFO Ticket group retry complete: recovered 43/50 actions, identified 2 missing ticket(s)
```

**Missing Ticket** (logged once per ticket):
```
WARN Ticket ID: 105310 not found - will skip future actions for this ticket
```

**Cache Write Failure** (non-critical):
```
WARN Failed to update cache with imported IDs: <error details>
```

### 7. Error Messages (ERROR)

**Import Failure**:
```
ERROR Failed to import action ID: 4108638 (ticket ID: 105310):
      Action object POST failed for action ID 4108638: status 400 Bad Request,
      error: "Ticket not found."
```

**Deserialization Failure**:
```
ERROR Failed to deserialize row in CSV file 'actions.csv': missing field `actiondate`
```

### 8. Sheet Completion (INFO)
```
INFO Completed sheet 1 of 9: CSV file 'file.csv' |
     10,000 processed, 500 imported, 9,500 skipped in 125.3s |
     avg sheet time: 125.3s
```

### 9. Final Summary (INFO + WARN)
```
INFO === Import Summary ===
INFO Total actions processed: 1,500,000
INFO Actions skipped (already exist): 1,400,000
INFO Actions successfully imported: 95,000
INFO Actions failed to import: 5,000
INFO === Performance Stats ===
INFO Total runtime: 86400.00s (1440.00m)
INFO Time per entry: 0.058s
INFO Entries per minute: 1041.7
INFO Average time per sheet: 9600.00s
WARN Files that could not be read: 2
```

## Logging Locations in Codebase

| Module | Purpose | Levels Used | Key Messages |
|--------|---------|-------------|--------------|
| `bin/main.rs` | High-level orchestration | INFO, ERROR | Startup, final summary |
| `setup.rs` | Auth, cache, file discovery | INFO, WARN | Config loaded, auth success, cache stats |
| `processor.rs` | Core import loop | INFO, WARN, ERROR | Progress, skip batches, success, errors |
| `summary.rs` | Statistics reporting | INFO, WARN | Final counts, performance metrics |
| `action.rs` (outbound) | API calls | WARN, ERROR | 401 retry, 504 timeout, POST failures |
| `client.rs` (inbound) | Report fetching | INFO, WARN, ERROR | Report fetch progress, ID counts |
| `auth/mod.rs` | Authentication | ERROR | Auth failures (critical) |

## Log Message Formatting Utilities

**Number Formatting** (`summary.rs`):
```rust
fn format_number(n: usize) -> String {
    n.to_string()
        .as_bytes()
        .rchunks(3)
        .rev()
        .map(std::str::from_utf8)
        .collect::<Result<Vec<&str>, _>>()
        .unwrap()
        .join(",")
}
// Example: 1500000 → "1,500,000"
```

**Duration Formatting**:
```rust
fn format_duration(secs: f64) -> String {
    let days = (secs / 86400.0).floor() as u64;
    let hours = ((secs % 86400.0) / 3600.0).floor() as u64;
    let minutes = ((secs % 3600.0) / 60.0).floor() as u64;
    let seconds = (secs % 60.0).floor() as u64;
    format!("{}d {}h {}m {}s", days, hours, minutes, seconds)
}
```

## Data Available for Enhanced Logging

### Currently Tracked But Not Structured
1. **Skipped Actions**:
   - `action_id` (String)
   - `ticket_id` (u32) - available in ActionObject
   - Reason: "already exists" (implicit)

2. **Failed Actions**:
   - Stored in `ProcessingStats.failed: Vec<(String, String)>`
   - Format: `(action_id, full_error_message)`
   - Error message includes ticket_id in text form

3. **Missing Tickets**:
   - Tracked in `HashSet<u32>` during processing
   - Logged once on first detection
   - Future actions silently skipped

4. **Successful Imports**:
   - `action_id` and `ticket_id` logged individually
   - IDs appended to `cache/imported_ids.txt`

### Timing Data Available
- Per-action import time (tracked for average calculation)
- Per-sheet processing time
- Total runtime
- Entries per minute
- Estimated time remaining

## Current Limitations for Troubleshooting

1. **No Grouping by Ticket**:
   - Can't easily answer: "Which tickets had the most failures?"
   - Can't track: "All actions for ticket X failed because..."

2. **Error Messages Unstructured**:
   - Error type buried in text (need regex to extract "not found", "400 Bad Request", etc.)
   - No categorization (missing ticket vs validation error vs network error)

3. **Skip Details Lost**:
   - Only know total skipped count
   - Can't identify: "Which specific actions were skipped for ticket Y?"

4. **Success Details Ephemeral**:
   - Logged to main log and cache file
   - No easy way to query: "Show me all successfully imported actions for ticket Z"

## Future Enhancement Opportunities

### Proposed: Structured Log Directory
```
log/
├── YYYY-MM-DD_HH-MM-SS/           # Log directory (not file)
│   ├── full.log                   # Current console/file output (unchanged)
│   ├── skipped.json               # Grouped by ticket_id
│   ├── errors.json                # Grouped by ticket_id → error_type → action_ids
│   ├── successes.json             # Grouped by ticket_id
│   └── summary.json               # Machine-readable statistics
```

### Benefits
1. **Troubleshooting**: Quickly identify which tickets had issues
2. **Reporting**: Generate ticket-level success/failure reports
3. **Auditing**: Know exactly which actions were processed for each ticket
4. **Debugging**: Categorize errors for pattern analysis
