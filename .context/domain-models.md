# Domain Models

## ActionObject
**Location**: `src/lib/domain/models/action_object.rs`

### Structure
```rust
pub struct ActionObject {
    pub ticket_id: u32,                    // Associated ticket/request ID
    pub actiondate: Option<NaiveDateTime>, // When action occurred (local time)
    pub outcome: String,                   // Action outcome
    pub note: String,                      // Action description/note
    pub actionwho: String,                 // Person who performed action
    pub action_id: ActionId,               // Unique identifier (custom field)
    pub _isimport: bool,                   // Always true for imports
}
```

### Deserialization Aliases
Flexible field name handling to support various CSV/Excel formats:

| Field | Aliases |
|-------|---------|
| `ticket_id` | requestId, RequestID, requestid, request_id, ticketid, TicketID, ticket_id |
| `actiondate` | ActionDate, ACTIONDATE, action_date, actionDate |
| `outcome` | Outcome, OUTCOME |
| `note` | Note, NOTE, notes, Notes, NOTES |
| `actionwho` | ActionWho, ACTIONWHO, action_who, who, Who, WHO |
| `action_id` | cfactionid, CFactionId, cdactionId, actionId, ActionID, action_id |

### Serialization to Halo API Format

**Custom Serialization**:
```rust
impl Serialize for ActionObject {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(11))?;

        // Convert Arizona time (UTC-7) to UTC
        let utc_datetime = self.actiondate
            .map(|dt| dt + Duration::hours(7));

        map.serialize_entry("__rowNum__", &None::<i32>)?;
        map.serialize_entry("_isimport", &self._isimport)?;
        map.serialize_entry("datetime", &utc_datetime)?;
        map.serialize_entry("actionwho", &self.actionwho)?;
        map.serialize_entry("cfactionid", &self.action_id.value())?;

        // Custom field structure for Halo
        map.serialize_entry("customfields", &vec![CustomField {
            name: "cfactionid".to_string(),
            value: self.action_id.value().to_string(),
        }])?;

        map.serialize_entry("note", &self.note)?;
        map.serialize_entry("note_html", &self.note)?;
        map.serialize_entry("outcome", &self.outcome)?;
        map.serialize_entry("requestid", &self.ticket_id)?;
        map.serialize_entry("result", &None::<String>)?;
        map.serialize_entry("ticket_id", &self.ticket_id)?;
        map.serialize_entry("who", &self.actionwho)?;

        map.end()
    }
}
```

**Example JSON Output**:
```json
{
  "__rowNum__": null,
  "_isimport": true,
  "datetime": "2024-12-24T08:23:45.000Z",
  "actionwho": "John Doe",
  "cfactionid": "4108638",
  "customfields": [
    {
      "name": "cfactionid",
      "value": "4108638"
    }
  ],
  "note": "Action note text",
  "note_html": "Action note text",
  "outcome": "Imported Note",
  "requestid": 174443,
  "result": null,
  "ticket_id": 174443,
  "who": "John Doe"
}
```

### Timezone Handling
**Arizona Time → UTC Conversion**:
- Input files contain dates in Arizona time (UTC-7, no DST)
- Serialization adds 7 hours to convert to UTC
- Example: 2024-12-24 01:23:45 (AZ) → 2024-12-24 08:23:45 (UTC)

### Date Parsing
Supports multiple formats during deserialization:
1. **ISO 8601**: `2024-12-24T01:23:45`
2. **Excel Serial Date**: Numeric value (e.g., 44927.5)
3. **Custom Formats**: Via chrono parsing

---

## ActionId
**Location**: `src/lib/domain/models/action_object.rs`

### Structure
```rust
pub struct ActionId(String);

impl ActionId {
    pub fn value(&self) -> &str {
        &self.0
    }

    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}
```

### Purpose
- Type-safe wrapper for action identifiers
- Ensures unique identification across imports
- Used as key for duplicate detection (via HashSet)

### Serialization
Converts to string for JSON output:
```rust
impl Serialize for ActionId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}
```

---

## ProcessingStats
**Location**: `src/lib/domain/importer/processor.rs`

### Structure
```rust
pub struct ProcessingStats {
    pub processed: usize,              // Total rows processed
    pub imported: usize,               // Successfully imported
    pub skipped: usize,                // Skipped (exist or missing ticket)
    pub failed: Vec<(String, String)>, // (action_id, error_message)
}
```

### Usage
Returned from `process_csv_file()` and `process_excel_file()` functions to track per-file statistics.

### Failed Actions Format
```rust
Vec<(String, String)>
// Example:
vec![
    ("4108638".to_string(),
     "Failed to import action ID: 4108638 (ticket ID: 105310): Action object POST failed: status 400 Bad Request, error: \"Ticket not found.\"".to_string())
]
```

**Note**: Error messages are unstructured strings containing:
- action_id
- ticket_id (embedded in text)
- HTTP status
- API error response

---

## ImportSummary
**Location**: `src/lib/domain/importer/summary.rs`

### Structure
```rust
pub struct ImportSummary {
    pub total_processed: usize,        // Sum of all processed actions
    pub total_imported: usize,         // Sum of all imported actions
    pub total_skipped: usize,          // Sum of all skipped actions
    pub total_failed: usize,           // Sum of all failed actions
    pub skipped_files: Vec<String>,    // Files that couldn't be read
    pub total_runtime_secs: f64,       // Total execution time
    pub sheet_times: Vec<f64>,         // Time per sheet/file
}
```

### Aggregation
Built by accumulating `ProcessingStats` from all files:
```rust
let mut summary = ImportSummary::default();
for stats in all_stats {
    summary.total_processed += stats.processed;
    summary.total_imported += stats.imported;
    summary.total_skipped += stats.skipped;
    summary.total_failed += stats.failed.len();
}
```

### Performance Metrics Calculation
```rust
// Time per entry (only counts actual imports)
let time_per_entry = if summary.total_imported > 0 {
    summary.total_runtime_secs / summary.total_imported as f64
} else {
    0.0
};

// Entries per minute
let entries_per_minute = if summary.total_runtime_secs > 0.0 {
    (summary.total_imported as f64 / summary.total_runtime_secs) * 60.0
} else {
    0.0
};

// Average time per sheet
let avg_sheet_time = if !summary.sheet_times.is_empty() {
    summary.sheet_times.iter().sum::<f64>() / summary.sheet_times.len() as f64
} else {
    0.0
};
```

---

## CacheData
**Location**: `src/lib/domain/importer/setup.rs`

### Structure
```rust
pub struct CacheData {
    pub action_ids: HashSet<String>,         // All known action IDs
    pub fetched_resources: HashSet<String>,  // Already-fetched report UUIDs
}
```

### Purpose
- **action_ids**: Fast O(1) duplicate lookup during processing
- **fetched_resources**: Tracks which reports have been fetched to avoid re-fetching

### Loading
Reads from two sources:
1. **cache/existing_action_ids.json**: Report IDs (JSON format)
2. **cache/imported_ids.txt**: Imported IDs (text format, one per line)

---

## ResourceCache
**Location**: `src/lib/domain/importer/setup.rs`

### Structure
```rust
#[derive(Serialize, Deserialize)]
pub struct ResourceCache {
    pub resource_id: String,       // Report UUID or "_imported"
    pub action_ids: Vec<String>,   // Action IDs from this resource
}
```

### JSON Format
```json
[
  {
    "resource_id": "aa637f8f-0e94-48e4-8881-8e1ff08445ec",
    "action_ids": ["12", "1234", "85656"]
  },
  {
    "resource_id": "9a887d53-85fa-4928-a450-9aece690ade2",
    "action_ids": ["3123312", "411", "4"]
  },
  {
    "resource_id": "_imported",
    "action_ids": ["999", "1000"]
  }
]
```

### Special Resource ID
**"_imported"**: Reserved ID for tracking actions imported by this application (not from reports).

---

## Config
**Location**: `src/lib/config.rs`

### Structure
```rust
pub struct Config {
    pub base_resource_url: Url,              // Halo instance URL
    pub token_url: Url,                      // OAuth2 token endpoint
    pub client_id: String,                   // OAuth2 client ID
    pub client_secret: String,               // OAuth2 client secret
    pub action_ids_resource_uuids: Vec<String>, // Report UUIDs
    pub action_id_custom_field_id: u32,      // Custom field ID
    pub log_level: Level,                    // Tracing log level
}
```

### Loading
Reads from environment variables (via `.env` file):
```rust
impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        dotenv::dotenv().ok();

        let base_url_str = std::env::var("BASE_RESOURCE_URL")?;
        let base_url = Url::parse(&base_url_str)?;

        let token_url = base_url.join("auth/token")?;

        let resource_paths = std::env::var("ACTION_IDS_RESOURCE_PATHS")?;
        let action_ids_resource_uuids: Vec<String> = resource_paths
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();

        // ... parse other fields

        Ok(Config { /* ... */ })
    }
}
```

---

## AuthToken
**Location**: `src/lib/outbound/client/auth/token.rs`

### Structure
```rust
pub struct AuthToken {
    access_token: String,      // Bearer token for API calls
    expires_at: i64,           // Unix timestamp (seconds since epoch)
}
```

### Methods
```rust
impl AuthToken {
    // Check if token is expired (with 30-second buffer)
    pub fn is_expired(&self) -> bool {
        let now = Utc::now().timestamp();
        now >= self.expires_at - 30
    }

    // Get formatted header value
    pub fn header_value(&self) -> String {
        format!("Bearer {}", self.access_token)
    }

    // Create from API response
    pub fn new(access_token: String, expires_in: i64) -> Self {
        let now = Utc::now().timestamp();
        Self {
            access_token,
            expires_at: now + expires_in,
        }
    }
}
```

### OAuth2 Response Format
```json
{
  "access_token": "eyJhbGc...",
  "token_type": "bearer",
  "expires_in": 3600
}
```

---

## CustomField
**Location**: `src/lib/domain/models/action_object.rs`

### Structure
```rust
#[derive(Serialize)]
struct CustomField {
    name: String,
    value: String,
}
```

### Purpose
Halo API requires custom fields in specific format:
```json
{
  "customfields": [
    {
      "name": "cfactionid",
      "value": "4108638"
    }
  ]
}
```

This allows Halo to populate the custom field that stores the unique action identifier for duplicate detection.
