# Halo Action Importer - Project Overview

## Purpose
Bulk import actions from CSV/Excel files into Halo ITSM, with intelligent duplicate detection, missing ticket handling, and resilient error recovery for large-scale imports (millions of records).

## Key Features
- **Duplicate Prevention**: Fetches existing action IDs from Halo reports to skip already-imported actions
- **Missing Ticket Detection**: Automatically detects and skips future actions for tickets not found in the system
- **Batched Processing**: Configurable batch sizes (1-N actions per API call) for improved throughput
- **Token Management**: Automatic OAuth2 token refresh with 401 retry logic for long-running imports
- **Incremental Caching**: Saves fetched IDs to disk to avoid re-fetching on restarts
- **Parallel Execution**: Supports running multiple instances on different input directories
- **Parse-Only Mode**: Validation mode that tests file parsing without making API calls
- **Performance Tracking**: Runtime stats, entries per minute, estimated completion time

## Architecture Pattern
**Layered Architecture** with clear separation of concerns:

```
┌─────────────────────────────────────────────────┐
│              CLI Entry Point                    │
│              (src/bin/main.rs)                  │
└─────────────────┬───────────────────────────────┘
                  │
┌─────────────────▼───────────────────────────────┐
│           Domain Layer (Business Logic)         │
│  ┌──────────────────────────────────────────┐  │
│  │  Importer (Orchestration)                │  │
│  │  • setup.rs: auth, cache, file discovery │  │
│  │  • processor.rs: CSV/Excel processing    │  │
│  │  • summary.rs: statistics reporting      │  │
│  └──────────────────────────────────────────┘  │
│  ┌──────────────────────────────────────────┐  │
│  │  Models                                   │  │
│  │  • ActionObject, ActionId                │  │
│  └──────────────────────────────────────────┘  │
└──────────────┬────────────────┬────────────────┘
               │                │
    ┌──────────▼───────┐   ┌───▼──────────────┐
    │ Inbound Layer    │   │ Outbound Layer   │
    │ (Data Sources)   │   │ (API Clients)    │
    ├──────────────────┤   ├──────────────────┤
    │ • ReportClient   │   │ • ActionClient   │
    │ • CsvReader      │   │ • AuthClient     │
    │ • ExcelReader    │   │                  │
    └──────────────────┘   └──────────────────┘
```

## Technology Stack
- **Language**: Rust (latest stable)
- **Async Runtime**: Tokio
- **HTTP Client**: Reqwest
- **Logging**: Tracing + Tracing-subscriber
- **CSV Parsing**: csv crate with streaming iterators
- **Excel Parsing**: calamine crate (in-memory processing)
- **Serialization**: serde + serde_json
- **Date/Time**: chrono (Arizona time → UTC conversion)

## Core Workflow

```
1. Parse CLI arguments (--batch-size, --only-parse, --input-path, --only-use-cache)
2. Initialize logging (dual output: file + console)
3. Setup phase:
   ├─ Load cached existing IDs from disk
   ├─ Authenticate with Halo OAuth2 API
   ├─ Fetch new existing IDs from Halo reports (unless --only-use-cache)
   └─ Discover CSV/Excel files in input directory
4. For each file:
   ├─ Create streaming iterator (CSV) or in-memory iterator (Excel)
   ├─ For each row:
   │  ├─ Deserialize to ActionObject
   │  ├─ Skip if action_id already exists
   │  ├─ Skip if ticket_id is in missing_tickets set
   │  ├─ Add to batch
   │  └─ If batch full: POST to Halo API (with 500ms rate limiting)
   └─ Log statistics (processed, imported, skipped, failed)
5. Display final summary with performance metrics
```

## Key Design Decisions

### Why Separate CSV and Excel Handling?
- **CSV**: Streaming iterator for memory efficiency (can handle multi-GB files)
- **Excel**: In-memory processing (calamine limitation, but Excel files are typically smaller)

### Why Dual Logging (File + Console)?
- **Console**: Real-time progress monitoring for operators
- **File**: Permanent record for auditing and troubleshooting

### Why HashSet for Duplicate Detection?
- O(1) lookup performance critical for millions of records
- Memory efficient: ~1M IDs = ~16-20MB RAM

### Why Batched Skip Messages?
- Reduces log clutter when thousands of consecutive actions are skipped
- Example: "Skipped 10,000 entries" vs 10,000 individual log lines

### Why Missing Ticket Tracking?
- Avoids repeated API calls for actions belonging to non-existent tickets
- Logs warning once per ticket, then silently skips future actions

### Why 500ms Delay Between API Calls?
- Rate limiting to avoid overwhelming Halo instance
- Configurable via code if needed

## CLI Arguments

| Argument | Short | Description |
|----------|-------|-------------|
| `--batch-size N` | `--bs N`, `--batch N` | Post N actions per API call (default: 1) |
| `--only-parse-inputs` | `--op` | Validate files without making import API calls |
| `--input-path PATH` | `--ip PATH` | Use custom input directory (default: `input/`) |
| `--only-use-cache` | `--oc` | Skip fetching reports, use only local cache |

## Environment Configuration
All configuration via `.env` file:
- `BASE_RESOURCE_URL`: Halo instance URL
- `CLIENT_ID`, `CLIENT_SECRET`: OAuth2 credentials
- `ACTION_IDS_RESOURCE_PATHS`: Comma-separated report UUIDs
- `ACTION_ID_CUSTOM_FIELD_ID`: Custom field ID for unique action identifier
- `LOG_LEVEL`: trace, debug, info (default), warn, error

## Performance Characteristics
- **Throughput**: ~1000-1500 entries/minute (single instance, batch size 1)
- **Scaling**: Linear with batch size and parallel instances
- **Memory**: ~50-100MB baseline + ~20MB per 1M cached IDs
- **Startup Time**: Depends on report size (3M IDs ≈ 30-60 seconds)

## Operational Patterns

### Running Multiple Instances
```bash
# Split files into subdirectories
cargo run --release -- --ip input/1 --bs 10 &
cargo run --release -- --ip input/2 --bs 10 &
cargo run --release -- --ip input/3 --bs 10 &
```

### Validation Before Import
```bash
cargo run --release -- --op  # Parse-only mode
```

### Resuming After Failure
- Cached IDs persist in `cache/` directory:
  - `cache/existing`: Comma-separated list of action IDs from Halo reports (updated 2026-01-27)
  - `cache/imported`: Line-separated list of action IDs imported by this tool
- Simply re-run: already-imported actions will be skipped
