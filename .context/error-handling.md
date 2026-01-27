# Error Handling Patterns

## Overall Strategy
**Resilient Continuation**: Errors are logged and collected, but processing continues. Only critical errors (auth, config) cause application exit.

---

## Error Categories

### 1. Critical Errors (Application Exit)
**Characteristics**: Unrecoverable, require user intervention

#### Authentication Failures
**Location**: `src/lib/outbound/client/auth/mod.rs`

```rust
if status == StatusCode::UNAUTHORIZED {
    error!("Authentication failed: invalid credentials (status: {})", status);
    bail!("Authentication failed: invalid credentials");
}
```

**Causes**:
- Invalid CLIENT_ID or CLIENT_SECRET
- Expired credentials
- Wrong token endpoint URL

**Effect**: Application exits immediately with error message

#### Configuration Errors
**Location**: `src/lib/config.rs`

```rust
let base_url_str = std::env::var(BASE_RESOURCE_URL_KEY)
    .with_context(|| format!("missing required environment variable: {}", BASE_RESOURCE_URL_KEY))?;

let log_level = match log_level_str.as_str() {
    "trace" => Level::TRACE,
    // ...
    _ => bail!("invalid log level '{}'. must be one of: trace, debug, info, warn, error", log_level_str),
};
```

**Causes**:
- Missing required environment variables
- Invalid URL format
- Invalid log level value
- Malformed report UUIDs

**Effect**: Application exits with descriptive error

---

### 2. Recoverable Errors (Retry Logic)

#### Token Expiration
**Location**: `src/lib/outbound/client/auth/token.rs`, `action.rs`

```rust
// Check before API call
pub fn is_expired(&self) -> bool {
    let now = Utc::now().timestamp();
    now >= self.expires_at - 30  // 30-second buffer
}

// Automatic refresh
if let Some(token) = token_guard.as_ref() && !token.is_expired() {
    return Ok(token.header_value());
}
let new_token = self.fetch_new_token().await?;
```

**Handling**:
1. Check expiration before each API call
2. Refresh token if expired (30-second buffer)
3. Continue with new token

**Effect**: Transparent to user, logged at DEBUG level

#### 401 Unauthorized
**Location**: `src/lib/outbound/client/action.rs`

```rust
if status == StatusCode::UNAUTHORIZED && attempt == 0 {
    warn!("Received 401 Unauthorized, refreshing token and retrying");
    auth_token = self.auth_client.get_valid_token().await?;
    continue; // Retry once
}
```

**Handling**:
1. Detect 401 response
2. Refresh token
3. Retry request once
4. If still fails, log error and continue

**Effect**: Single retry, logged as WARN

#### 504 Gateway Timeout
**Location**: `src/lib/outbound/client/action.rs`

```rust
if status == StatusCode::GATEWAY_TIMEOUT {
    warn!("Received 504 Gateway Timeout, waiting 1 minute before retrying");
    tokio::time::sleep(Duration::from_secs(60)).await;
    auth_token = self.auth_client.get_valid_token().await?;
    break; // Retry outer loop
}
```

**Handling**:
1. Detect 504 response
2. Wait 60 seconds
3. Refresh token
4. Retry indefinitely (outer loop continues)

**Effect**: Handles temporary Halo instance overload

---

### 3. Data Errors (Skip and Continue)

#### Deserialization Errors
**Location**: `src/lib/domain/importer/processor.rs`

```rust
let action = match action_result {
    Ok(a) => a,
    Err(e) => {
        error!("Failed to deserialize row in CSV file '{}': {}", file_name, e);
        stats.failed.push(("unknown".to_string(), e.to_string()));
        continue;
    }
};
```

**Causes**:
- Missing required fields
- Invalid date format
- Type mismatch (string vs number)
- Malformed JSON (Excel conversion)

**Effect**:
- Row skipped
- ERROR logged with file name and reason
- Count added to failed total
- Processing continues with next row

#### Missing Fields
**Example Error**:
```
ERROR Failed to deserialize row in CSV file 'actions.csv': missing field `actiondate` at line 1234
```

**Common Fields**:
- `actiondate` (required)
- `ticket_id` / `requestId` (required)
- `action_id` / `cfactionid` (required)
- `actionwho` / `who` (required)
- `note` (required)

---

### 4. API Import Errors (Log and Continue)

#### Missing Ticket (400 Bad Request)
**Location**: `src/lib/domain/importer/processor.rs`

```rust
match action_client.post_action_objects(batch.clone()).await {
    Err(e) => {
        let error_str = e.to_string();

        // Special handling for missing tickets
        if error_str.contains("not found") || error_str.contains("404") {
            let ticket_id = batch[0].ticket_id;
            missing_tickets.insert(ticket_id);
            warn!("Ticket ID: {} not found - will skip future actions for this ticket", ticket_id);
        }

        // Log error for each action in batch
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
```

**Optimization**: Once a ticket is marked as missing, future actions for that ticket are skipped **without** making API calls.

**Effect**:
- First error: Logged as WARN + ERROR
- Future actions: Silently skipped (counted in skipped total)
- Prevents repeated 400 errors for same ticket

#### Other API Errors
**Causes**:
- Validation errors (invalid field values)
- Permission errors (insufficient API access)
- Rate limiting (too many requests)
- Network errors (connection timeout)

**Effect**:
- ERROR logged with action_id, ticket_id, and full error details
- Action added to failed list
- Processing continues with next batch

---

### 5. File Read Errors (Skip File)

**Location**: `src/bin/main.rs`

```rust
match process_csv_file(...).await {
    Ok(stats) => {
        all_stats.push(stats);
    }
    Err(e) => {
        error!("Failed to read file {:?}: {}", file_name, e);
        skipped_files.push(format!("{:?}: {}", file_name, e));
    }
}
```

**Causes**:
- File not found
- Permission denied
- Corrupted file format
- Invalid Excel structure
- CSV encoding issues

**Effect**:
- ERROR logged with file name and reason
- File added to skipped_files list
- Processing continues with next file
- Summary shows skipped file count

---

### 6. Cache Errors (Warn and Continue)

#### Cache Write Failures
**Location**: `src/lib/domain/importer/processor.rs`, `setup.rs`

```rust
if let Err(e) = append_imported_ids_to_cache(&imported_ids) {
    warn!("Failed to update cache with imported IDs: {}", e);
}
```

**Causes**:
- Disk full
- Permission denied
- Lock file contention (multiple instances)
- Invalid JSON structure

**Effect**:
- WARN logged
- Processing continues
- Degraded operation: next run may re-process some actions

#### Cache Read Failures
**Location**: `src/lib/domain/importer/setup.rs`

```rust
pub fn read_cached_ids() -> anyhow::Result<CacheData> {
    // If file doesn't exist, return empty cache
    if !Path::new("cache/existing_action_ids.json").exists() {
        return Ok(CacheData::default());
    }

    // If parsing fails, log and return empty cache
    let caches: Vec<ResourceCache> = match serde_json::from_reader(file) {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to parse cache file: {}", e);
            return Ok(CacheData::default());
        }
    };
    // ...
}
```

**Effect**:
- WARN logged
- Empty cache used (all reports will be fetched)
- Processing continues

---

## Error Message Structure

### Format Convention
All error messages include context for troubleshooting:

**Pattern**: `"Failed to [action] [resource]: [details]"`

**Examples**:
```
Failed to import action ID: 4108638 (ticket ID: 105310): Action object POST failed: status 400 Bad Request, error: "Ticket not found."

Failed to deserialize row in CSV file 'actions.csv': missing field `actiondate` at line 1234

Failed to read file "corrupted.xlsx": calamine error: invalid zip structure

Failed to update cache with imported IDs: permission denied
```

### Components
1. **Action**: What was being attempted
2. **Resource**: What was being acted upon (action ID, file, ticket)
3. **Details**: Underlying cause (HTTP status, missing field, system error)

---

## Error Storage and Reporting

### During Processing
```rust
pub struct ProcessingStats {
    pub failed: Vec<(String, String)>, // (action_id, error_message)
}
```

**Storage**:
```rust
stats.failed.push((
    action.action_id.value().to_string(),
    full_error_message
));
```

### In Summary
```rust
pub struct ImportSummary {
    pub total_failed: usize,           // Count of failed actions
    pub skipped_files: Vec<String>,    // File names + errors
}

// Aggregation
for stats in all_stats {
    summary.total_failed += stats.failed.len();
}

// Logging
info!("Actions failed to import: {}", format_number(summary.total_failed));
if !summary.skipped_files.is_empty() {
    warn!("Files that could not be read: {}", summary.skipped_files.len());
}
```

**Current Limitation**: Individual error details are not preserved in summary (only count).

---

## Error Recovery Patterns

### Retry with Backoff
```rust
// 401: Immediate retry after token refresh
if status == StatusCode::UNAUTHORIZED && attempt == 0 {
    auth_token = self.auth_client.get_valid_token().await?;
    continue;
}

// 504: 60-second backoff before retry
if status == StatusCode::GATEWAY_TIMEOUT {
    tokio::time::sleep(Duration::from_secs(60)).await;
    auth_token = self.auth_client.get_valid_token().await?;
    break; // Retry outer loop
}
```

### Missing Ticket Optimization
```rust
// Check before API call
if missing_tickets.contains(&action.ticket_id) {
    stats.skipped += 1;
    pending_skips += 1;
    continue; // Skip API call entirely
}

// Add to set on first failure
if error_str.contains("not found") {
    missing_tickets.insert(ticket_id);
    warn!("Ticket ID: {} not found - will skip future actions", ticket_id);
}
```

### Batch Error Handling
```rust
// When batch fails, log error for EACH action
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
```

**Implication**: Batch failures affect all actions in batch (no partial success).

---

## Error Detection Patterns

### HTTP Status Checks
```rust
match status {
    StatusCode::UNAUTHORIZED if attempt == 0 => { /* retry */ }
    StatusCode::GATEWAY_TIMEOUT => { /* backoff + retry */ }
    _ if !status.is_success() => {
        let error_text = response.text().await?;
        bail!("API call failed: status {}, error: {}", status, error_text);
    }
    _ => { /* success */ }
}
```

### Error Message Inspection
```rust
let error_str = e.to_string();
if error_str.contains("not found") || error_str.contains("404") {
    // Missing ticket handling
}
```

**Limitation**: String matching is brittle (depends on Halo API error format).

---

## Debugging Aids

### Log Levels for Troubleshooting

| Issue | Recommended Level | What to Look For |
|-------|-------------------|------------------|
| Authentication | DEBUG | Token fetch, refresh timing |
| API failures | INFO/ERROR | HTTP status, error messages |
| Deserialization | ERROR | Missing fields, type mismatches |
| Performance | INFO | Progress updates, time per entry |
| Cache issues | WARN | Cache read/write failures |

### Error Context
All errors include sufficient context:
- **File name**: Which file caused the error
- **Action ID**: Which action failed
- **Ticket ID**: Associated ticket
- **Row number**: For deserialization errors (CSV/Excel line)
- **HTTP status**: For API errors
- **API error message**: Halo's error response

### Example Debugging Session
```
# Enable DEBUG logging
LOG_LEVEL=debug cargo run --release

# Look for:
ERROR Failed to import action ID: 4108638 (ticket ID: 105310): ...
  → Action 4108638 failed
  → Belongs to ticket 105310
  → Check ticket status in Halo

ERROR Failed to deserialize row in CSV file 'actions.csv': missing field `actiondate` at line 1234
  → Row 1234 in actions.csv
  → Missing actiondate field
  → Check CSV structure

WARN Ticket ID: 105310 not found - will skip future actions for this ticket
  → Ticket 105310 doesn't exist in Halo
  → All future actions for this ticket will be skipped
```

---

## Future Enhancement Opportunities

### Structured Error Storage
Current: `Vec<(String, String)>` - action_id + unstructured error message

Proposed: Structured error types
```rust
pub enum ImportError {
    MissingTicket { action_id: String, ticket_id: u32 },
    ValidationError { action_id: String, ticket_id: u32, field: String, details: String },
    NetworkError { action_id: String, ticket_id: u32, status: u16, message: String },
    DeserializationError { row: usize, field: String, details: String },
}
```

**Benefits**:
- Easier error grouping by type
- Better reporting (e.g., "15 missing tickets, 3 validation errors")
- Machine-readable error logs (JSON export)

### Partial Batch Success
Current: Entire batch fails if API returns error

Proposed: Per-action error handling
- API returns which actions succeeded/failed in batch
- Retry only failed actions
- Better success rate for large batches

### Automatic Retry Queue
Current: Failed actions are logged but not retried

Proposed: Retry queue
- Failed actions written to `failed_actions.json`
- Separate retry command: `cargo run --retry-failed`
- Exponential backoff for persistent failures
