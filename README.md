# Halo Action Importer

A Rust application for bulk importing actions into Halo. Processes CSV and Excel files, skips existing actions based on unique identifiers, and provides comprehensive logging and performance statistics.

## Features

- Bulk import actions from CSV and Excel files
- Automatic duplicate detection using unique action identifiers
- Missing ticket detection - skips future actions for tickets not found in system
- Automatic token refresh and 401 retry logic for long-running imports
- Incremental processing to handle large files efficiently
- Comprehensive logging with configurable log levels and timestamps
- Performance statistics (runtime, entries per minute, time per entry, estimated time remaining)
- Parse-only mode for validation without API calls
- Custom input directory support for parallel execution
- Batched skip messages to reduce log clutter
- Progress tracking with configurable update frequencies
- Error handling that continues processing on failures
- Timezone conversion from Arizona time (UTC-7) to UTC for API calls

## Requirements

- Halo instance with API access
- Actions to import in Excel/CSV format, formatted correctly per Halo API documentation
- Actions must have a unique identifier (custom field) outside of what Halo assigns at creation
  - This allows the application to check which actions have already been imported
- Rust toolchain (latest stable version)

## Configuration

Create a `.env` file in the project root with the following environment variables:

```env
# Configuration
LOG_LEVEL = debug

# Instance
BASE_RESOURCE_URL = https://example.haloitsm.com/

# API Application
CLIENT_ID = client-id-goes-here
CLIENT_SECRET = super-secret-client-secret-goes-here

# Actions - just the UUIDs, application builds full URL
ACTION_IDS_RESOURCE_PATHS = aa637f8f-0e94-48e4-8881-8e1ff08445ec,9a887d53-85fa-4928-a450-9aece690ade2,385ac9e1-4679-4e43-88a7-3c02876cab25
ACTION_ID_CUSTOM_FIELD_ID = 123
```

### Environment Variables

- `BASE_RESOURCE_URL` - Base URL of your Halo instance (include trailing slash)
- `CLIENT_ID` - OAuth2 client ID for API authentication
- `CLIENT_SECRET` - OAuth2 client secret for API authentication
- `ACTION_IDS_RESOURCE_PATHS` - UUID(s) of report(s) that return existing action IDs. Just the UUIDs, the application builds the full URL. Can be a single UUID or comma-separated list (e.g., `aa637f8f-0e94-48e4-8881-8e1ff08445ec,9a887d53-85fa-4928-a450-9aece690ade2`). **CRITICAL:** For large datasets (3M+ IDs), use multiple reports to avoid timeouts. See `sql/` directory for query templates.
- `ACTION_ID_CUSTOM_FIELD_ID` - Custom field ID used to store the unique action identifier (numeric value)
- `LOG_LEVEL` - Logging level (trace, debug, info, warn, error). Defaults to `info` if not specified.

## Usage

### Standard Import Mode

Place your CSV and Excel files in the `input/` directory, then run:

```bash
cargo run --release
```

The application will:
1. Authenticate with the Halo API
2. Fetch existing action IDs from the configured report(s)
3. Process all CSV and Excel files in the `input/` directory
4. Skip actions that already exist
5. Import new actions
6. Generate a log file in the `log/` directory with a UTC timestamp

### Custom Input Directory

To process files from a different directory:

```bash
cargo run --release -- --input-path input/1
# or use short form
cargo run --release -- --ip input/1
```

This is useful for running multiple instances in parallel on different input directories. You can organize files into subdirectories like `input/1/`, `input/2/`, etc., and run separate instances for each.

### Parse-Only Mode

To validate files without making API calls:

```bash
cargo run --release -- --only-parse-inputs
# or use short form
cargo run --release -- --op
```

This mode:
- Authenticates with the Halo API
- Fetches and parses existing action IDs from all configured reports (tests report connectivity)
- Parses all files and validates data structure
- Shows which actions would be imported or skipped (based on fetched IDs)
- Does not make any import API calls
- Uses reduced logging frequency (every 10,000 entries or 5 seconds)
- Shows success message if all actions parse successfully

This is useful for:
- Testing report configuration and connectivity
- Validating file formats before running a full import
- Verifying that existing ID reports are working correctly

### Batch Mode

Post multiple actions per API request (default is 1):

```bash
cargo run --release -- --batch-size 10
# or use short forms
cargo run --release -- --batch 10
cargo run --release -- --bs 10
```

Batch mode groups actions into batches of the specified size before posting to the API. This significantly improves throughput by reducing the number of API calls and network overhead.

### Cache-Only Mode

Skip fetching existing IDs from reports and use only the local cache:

```bash
cargo run --release -- --only-use-cache
# or use short form
cargo run --release -- --oc
```

This is useful when you're confident the cache is up to date and want to skip the time-consuming report fetching.

#### Cache Files (Updated 2026-01-27)

The application maintains two cache files in the `cache/` directory:

1. **`cache/existing.json`** - JSON format tracking which resources (reports) have been fetched and their action IDs
   ```json
   [
     {
       "resource_id": "aa637f8f-0e94-48e4-8881-8e1ff08445ec",
       "action_ids": ["12", "1234", "85656"]
     },
     {
       "resource_id": "9a887d53-85fa-4928-a450-9aece690ade2",
       "action_ids": ["3123312", "411", "4"]
     }
   ]
   ```
   This format allows the application to skip already-fetched resources on subsequent runs, avoiding redundant API calls.

2. **`cache/imported`** - Line-separated list of imported action IDs (one per line)
   ```
   999
   1000
   1001
   ```

This allows the application to:
- Skip actions that already exist in Halo
- Skip already-fetched report resources to avoid redundant API calls
- Skip actions that have been imported by this tool
- Resume from where it left off after interruption

### Parallel Execution

You can run multiple instances on different input directories:

```bash
# Run multiple instances in parallel (different subdirectories with batch mode)
cargo run --release -- --ip input/1 --bs 10 &
cargo run --release -- --ip input/2 --bs 10 &
cargo run --release -- --ip input/3 --bs 10 &
```

Simply split your files into separate directories and run one instance per directory.

## File Format

Input files should be placed in the `input/` directory and can be:
- CSV files (`.csv`)
- Excel files (`.xlsx`, `.xls`)

Each file should contain action data with the following required fields:
- `CFactionId` or `actionId` - Unique identifier for the action
- `requestId` or `ticket_id` - Associated ticket/request ID
- `actionWho` - Person who performed the action
- `note` - Action notes/description
- `actionDate` - Date/time of the action (ISO 8601 format or Excel serial date)
- `outcome` - Optional, defaults if not provided

Additional fields are allowed and will be ignored during deserialization.

## Logging (Updated 2026-01-27)

### Directory-Based Logging

Each run creates a timestamped directory containing multiple output files:

**Directory**: `log/YYYY-MM-DD_HH-MM-SS/`

**Files generated**:
- **`full.log`** - Complete log of all messages (also shown on console)
- **`retry.csv`** - Failed actions that need retry (if any failures occurred)
- **`summary.json`** - Machine-readable statistics for the run

All log entries include RFC 3339 timestamps for precise tracking.

### Log Levels

- `trace` - Most verbose, includes all internal operations
- `debug` - Debug information and detailed flow
- `info` - General information, progress updates, summaries (default)
- `warn` - Warnings and non-critical errors (e.g., missing tickets)
- `error` - Errors that prevent processing

### Progress Tracking (Updated 2026-01-27)

Progress is shown through success logs for each import. **No periodic progress updates** - each successful import shows current position and estimated time remaining.

Every successful import includes:
- Current position (action X/Y or batch X/Y)
- Action/ticket IDs
- Cumulative skip count
- Average time per action
- Estimated time remaining (formatted as days/hours/minutes/seconds)

### Log Messages

**Skip Messages**: Consecutive skips are batched into single messages:
```
Skipped 1,234 entries (already exist)
```

**Success Messages** (Updated 2026-01-27): Each successful import shows comprehensive progress:

For batch size of 1 (default):
```
INFO Imported action 1/10000 (ID: 12345, ticket: 67890) | 50 total skipped | 0.50s/row | ETA: 1h 23m 15s
```

When using `--batch-size` mode with batch size > 1:
```
INFO Imported batch 1/200 (50 actions, tickets: 67890, 67891, 67892) | 50 total skipped | 0.05s/action | ETA: 8m 15s
```

**Note**: All ticket IDs are shown (no truncation). Ticket IDs are deduplicated since multiple actions can belong to the same ticket.

**Missing Tickets**: When a ticket is not found, it's logged once and future actions for that ticket are skipped:
```
WARN Ticket ID: 67890 not found - will skip future actions for this ticket
```

**Error Messages**: Include both action ID and ticket ID:
```
ERROR Failed to import action ID: 12345 (ticket ID: 67890): <error details>
```

## Output

The application provides a comprehensive summary including:

- Total actions processed
- Actions skipped (already exist)
- Actions successfully imported
- Actions failed to import
- Performance statistics:
  - Total runtime (seconds and minutes)
  - Time per entry
  - Entries per minute
  - Average time per sheet

## Error Handling (Updated 2026-01-27)

The application is designed to be resilient:
- Deserialization errors are logged and the row is skipped
- API errors are logged and processing continues
- File read errors are logged and the file is skipped
- Missing tickets are detected and future actions for them are automatically skipped
- Token expiration is handled automatically with refresh and retry
- 401 Unauthorized responses trigger automatic token refresh and retry
- Network errors, connection failures, and 504 Gateway Timeouts retry indefinitely
- All errors are collected and reported in the final summary

### Smart Batch Retry (2026-01-27)

When a batch fails with "not found" error, the application uses **ticket-grouped retry**:
1. Groups all actions in the failed batch by `ticket_id`
2. Retries each ticket group independently
3. Successfully imports actions for valid tickets
4. Marks all actions for missing tickets as failed
5. Adds missing tickets to skip set

This maximizes successful imports while minimizing API calls. For example, a batch of 50 actions with 2 missing tickets will make ~5-10 API calls instead of 50 individual retries.

### Token Management

The application automatically manages OAuth2 tokens:
- Checks token expiration before each API call
- Refreshes tokens when expired (with 30-second buffer)
- Retries requests once on 401 Unauthorized
- Handles long-running imports without manual intervention

## Project Structure

```
src/
├── bin/
│   └── main.rs              # Application entry point
└── lib/
    ├── config.rs            # Configuration management
    ├── domain/
    │   ├── importer/        # Core import logic
    │   │   ├── setup.rs     # Logging, auth, file discovery
    │   │   ├── processor.rs # CSV/Excel processing
    │   │   └── summary.rs   # Summary reporting
    │   └── models/          # Domain models
    ├── inbound/             # Data input handling
    │   ├── client.rs        # Report client for existing IDs
    │   └── file/            # File readers (CSV, Excel)
    └── outbound/            # API clients
        └── client/
            ├── action.rs    # Action import client
            └── auth/        # Authentication client
```

## Building

```bash
# Development build
cargo build

# Release build (recommended for production)
cargo build --release

# Run tests
cargo test
```

## License

This project is licensed under the MIT License - see the LICENSE file for details.

MIT License is a permissive open source license that allows anyone to use, modify, and distribute this software for any purpose, including commercial use, with minimal restrictions.
