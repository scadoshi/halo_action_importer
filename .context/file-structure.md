# File Structure and Module Organization

## Directory Layout

```
halo_action_importer/
├── .env                            # Configuration (not in git)
├── .gitignore                      # Git ignore rules
├── Cargo.toml                      # Rust dependencies and metadata
├── Cargo.lock                      # Locked dependency versions
├── README.md                       # Project documentation
├── LICENSE                         # MIT License
│
├── .context/                       # Project context documentation
│   ├── project-overview.md         # High-level architecture
│   ├── logging-system.md           # Logging implementation
│   ├── domain-models.md            # Data structures
│   ├── processing-flow.md          # Import workflow
│   ├── error-handling.md           # Error patterns
│   ├── api-clients.md              # API communication
│   └── file-structure.md           # This file
│
├── cache/                          # Runtime cache (tracked in git as empty dir)
│   ├── existing_action_ids.json   # Cached report IDs (JSON)
│   └── imported_ids.txt            # Imported action IDs (text)
│
├── input/                          # Input files (CSV/Excel)
│   ├── *.csv                       # CSV files
│   ├── *.xlsx                      # Excel files (new format)
│   ├── *.xls                       # Excel files (old format)
│   ├── 1/                          # Optional subdirectories for parallel execution
│   ├── 2/
│   └── 3/
│
├── log/                            # Log files
│   └── YYYY-MM-DD_HH-MM-SS.log    # Timestamped log files (UTC)
│
├── sql/                            # SQL query templates (documentation)
│   └── existing_action_ids_query.sql  # Example query for Halo reports
│
├── src/                            # Source code
│   ├── bin/
│   │   └── main.rs                 # Application entry point
│   └── lib/
│       ├── lib.rs                  # Library root (module exports)
│       ├── config.rs               # Configuration management
│       │
│       ├── domain/                 # Business logic layer
│       │   ├── mod.rs              # Domain module exports
│       │   │
│       │   ├── importer/           # Core import orchestration
│       │   │   ├── mod.rs          # Importer module exports
│       │   │   ├── setup.rs        # Setup: logging, auth, cache, files
│       │   │   ├── processor.rs    # Core processing: CSV/Excel import
│       │   │   └── summary.rs      # Summary statistics and reporting
│       │   │
│       │   └── models/             # Domain models
│       │       ├── mod.rs          # Models module exports
│       │       └── action_object.rs  # ActionObject, ActionId
│       │
│       ├── inbound/                # Data input layer
│       │   ├── mod.rs              # Inbound module exports
│       │   ├── client.rs           # ReportClient (fetch existing IDs)
│       │   │
│       │   └── file/               # File readers
│       │       ├── mod.rs          # File module exports
│       │       ├── csv.rs          # CSV reader with streaming iterator
│       │       └── excel.rs        # Excel reader with in-memory iterator
│       │
│       └── outbound/               # External API layer
│           ├── mod.rs              # Outbound module exports
│           │
│           └── client/             # API clients
│               ├── mod.rs          # Client module exports
│               ├── action.rs       # ActionClient (POST actions)
│               │
│               └── auth/           # Authentication
│                   ├── mod.rs      # AuthClient
│                   └── token.rs    # AuthToken management
│
└── target/                         # Build artifacts (not in git)
    ├── debug/                      # Debug builds
    └── release/                    # Release builds (optimized)
```

---

## Module Hierarchy

### Root Module (`lib.rs`)
**Purpose**: Library entry point, re-exports public API

```rust
pub mod config;
pub mod domain;
pub mod inbound;
pub mod outbound;
```

**Public API**:
- `Config` - Configuration management
- `setup()` - Initialize application
- `process_csv_file()`, `process_excel_file()` - File processors
- `ActionClient`, `AuthClient`, `ReportClient` - API clients

---

### config Module
**File**: `src/lib/config.rs`
**Purpose**: Load and validate configuration from environment variables

**Public Types**:
- `Config` - Configuration struct

**Responsibilities**:
- Load `.env` file
- Parse environment variables
- Validate URLs and UUIDs
- Set defaults (log level)

**Dependencies**:
- `dotenv` - Load `.env` file
- `url::Url` - Parse and validate URLs
- `tracing::Level` - Log level types

---

### domain Module
**Location**: `src/lib/domain/`
**Purpose**: Business logic and orchestration

#### domain/importer
**Purpose**: Core import orchestration

**Files**:
1. **setup.rs**
   - Setup logging (dual output: file + console)
   - Load cached IDs from disk
   - Authenticate with Halo
   - Fetch existing IDs from reports
   - Discover input files
   - Cache management (read/write)

2. **processor.rs**
   - Process CSV files (streaming)
   - Process Excel files (in-memory)
   - Batch management
   - Progress tracking
   - Skip counting
   - Error collection

3. **summary.rs**
   - Aggregate statistics from all files
   - Calculate performance metrics
   - Format and log final summary

**Key Functions**:
- `setup()` - Initialize application
- `process_csv_file()` - Process single CSV file
- `process_excel_file()` - Process single Excel file
- `log_summary()` - Display final statistics

#### domain/models
**Purpose**: Domain data structures

**Files**:
1. **action_object.rs**
   - `ActionObject` - Core action model
   - `ActionId` - Type-safe action identifier
   - Custom serialization for Halo API
   - Timezone conversion (Arizona → UTC)
   - Field aliases for deserialization

**Key Types**:
- `ActionObject` - Action with ticket_id, note, date, etc.
- `ActionId` - Wrapper for unique identifier
- `ProcessingStats` - Per-file statistics
- `CustomField` - Halo custom field structure

---

### inbound Module
**Location**: `src/lib/inbound/`
**Purpose**: Read data from external sources

#### inbound/client
**File**: `src/lib/inbound/client.rs`
**Purpose**: Fetch existing action IDs from Halo reports

**Key Types**:
- `ReportClient` - HTTP client for report API
- `ReportRow` - Deserialization helper

**Responsibilities**:
- Fetch action IDs from report UUIDs
- Parse JSON response
- Filter out null IDs
- Handle authentication

#### inbound/file
**Purpose**: Read CSV and Excel files

**Files**:
1. **csv.rs**
   - `csv_action_iter()` - Create streaming iterator
   - `CsvActionIterator` - Iterator over ActionObjects
   - Count rows (first pass)
   - Deserialize with flexible field names

2. **excel.rs**
   - `excel_action_iter()` - Create in-memory iterator
   - `ExcelActionIterator` - Iterator over ActionObjects
   - Load all rows into memory
   - Handle Excel serial dates
   - Convert rows to JSON for deserialization

**Key Types**:
- `CsvActionIterator` - Streaming CSV iterator
- `ExcelActionIterator` - In-memory Excel iterator

**Design Choice**: CSV uses streaming for memory efficiency, Excel loads all rows (calamine limitation).

---

### outbound Module
**Location**: `src/lib/outbound/`
**Purpose**: Communicate with external APIs

#### outbound/client
**Files**:
1. **action.rs**
   - `ActionClient` - POST actions to Halo
   - Batch import support
   - 500ms rate limiting
   - Retry logic (401, 504)
   - Error extraction from responses

2. **auth/mod.rs**
   - `AuthClient` - OAuth2 authentication
   - Token caching
   - Automatic token refresh
   - Thread-safe token storage

3. **auth/token.rs**
   - `AuthToken` - Token model
   - Expiration checking (30s buffer)
   - Header value formatting

**Key Types**:
- `ActionClient` - Action import client
- `AuthClient` - Authentication client
- `AuthToken` - Token with expiration

**Shared Pattern**: All clients share single AuthClient instance via `Arc<AuthClient>`.

---

## Binary Entry Point

### bin/main.rs
**Purpose**: CLI entry point and orchestration

**Responsibilities**:
1. Parse CLI arguments
2. Load configuration
3. Call `setup()` from domain layer
4. Process all files
5. Aggregate statistics
6. Display summary

**CLI Parsing**: Manual parsing (no dependencies like clap)

**Flow**:
```
parse args → load config → setup → process files → log summary
```

---

## Module Dependencies

### Dependency Graph
```
bin/main
    ↓
domain/importer/setup ← config
    ↓                     ↓
domain/importer/processor → outbound/client/action
    ↓                           ↓
inbound/file/{csv,excel}    outbound/client/auth
    ↓                           ↓
domain/models/action_object     outbound/client/auth/token
```

### External Dependencies (Cargo.toml)

**HTTP & Async**:
- `tokio` - Async runtime
- `reqwest` - HTTP client

**Serialization**:
- `serde` - Serialization framework
- `serde_json` - JSON support

**File Parsing**:
- `csv` - CSV parsing
- `calamine` - Excel parsing

**Logging**:
- `tracing` - Structured logging
- `tracing-subscriber` - Log output formatting

**Error Handling**:
- `anyhow` - Error handling and context

**Date/Time**:
- `chrono` - Date/time handling (timezone conversion)

**Utilities**:
- `dotenv` - Load `.env` files
- `url` - URL parsing

---

## File Naming Conventions

### Source Files
- **snake_case**: `action_object.rs`, `csv.rs`, `excel.rs`
- **mod.rs**: Module root (re-exports)

### Runtime Files
- **Timestamps**: `YYYY-MM-DD_HH-MM-SS.log` (UTC, seconds precision)
- **Cache**: JSON for structured data, text for simple lists

### Configuration
- **Environment**: `.env` (not in git)
- **Documentation**: `README.md`, `LICENSE`

---

## Build Artifacts

### target/
**Not in git** (excluded via `.gitignore`)

**Contents**:
- `debug/` - Debug builds (fast compile, slow runtime)
- `release/` - Release builds (slow compile, fast runtime)
- `deps/` - Compiled dependencies
- `incremental/` - Incremental compilation cache

**Build Commands**:
```bash
cargo build              # Debug build
cargo build --release    # Release build (recommended for production)
cargo run                # Build + run (debug)
cargo run --release      # Build + run (release)
```

---

## Cache Files

### cache/existing_action_ids.json
**Format**: JSON array of ResourceCache objects

```json
[
  {
    "resource_id": "aa637f8f-0e94-48e4-8881-8e1ff08445ec",
    "action_ids": ["12", "1234", "85656"]
  },
  {
    "resource_id": "_imported",
    "action_ids": ["999", "1000"]
  }
]
```

**Purpose**: Track which reports have been fetched and their IDs

**Updates**: Appended incrementally as reports are fetched

### cache/imported_ids.txt
**Format**: Plain text, one ID per line

```
999
1000
1001
```

**Purpose**: Track action IDs imported by this application

**Updates**: Appended after each successful import

---

## Log Files

### log/YYYY-MM-DD_HH-MM-SS.log
**Format**: Plain text with RFC 3339 timestamps

```
2024-12-24T18:43:07.123456Z INFO Starting Halo action importer
2024-12-24T18:43:07.234567Z INFO Authentication successful
2024-12-24T18:43:08.345678Z INFO Processing sheet 1 of 9: CSV file 'actions.csv' (10000 rows)
```

**Naming**: UTC timestamp with seconds precision (avoids collisions)

**Rotation**: None (new file per run)

**Cleanup**: Manual (no automatic deletion)

---

## Input File Organization

### Single Directory (Default)
```
input/
├── actions_1.csv
├── actions_2.csv
└── tickets.xlsx
```

**Usage**: `cargo run --release`

### Subdirectories (Parallel Execution)
```
input/
├── 1/
│   ├── batch_1_a.csv
│   └── batch_1_b.csv
├── 2/
│   ├── batch_2_a.csv
│   └── batch_2_b.csv
└── 3/
    ├── batch_3_a.csv
    └── batch_3_b.csv
```

**Usage**:
```bash
cargo run --release -- --ip input/1 &
cargo run --release -- --ip input/2 &
cargo run --release -- --ip input/3 &
```

---

## Module Visibility

### Public API (Exported from lib.rs)
- `Config::from_env()`
- `setup()`
- `process_csv_file()`, `process_excel_file()`
- `log_summary()`
- `ActionClient`, `AuthClient`, `ReportClient`

### Internal Implementation (Not exported)
- File iterators (CSV, Excel)
- Cache management functions
- Progress logging helpers
- Error handling utilities

---

## Testing Structure
**Note**: No test files currently in codebase (integration tests via manual runs).

**Future Structure** (if tests added):
```
src/
└── lib/
    └── domain/
        └── models/
            ├── action_object.rs
            └── tests/
                └── action_object_test.rs
```

Or:
```
tests/
├── integration/
│   ├── csv_processing_test.rs
│   └── excel_processing_test.rs
└── fixtures/
    ├── sample.csv
    └── sample.xlsx
```
