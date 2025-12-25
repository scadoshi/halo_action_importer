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
- Reverse mode for processing files from bottom to top
- Half mode for processing only half the files (useful for parallel execution)
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

# Actions
ACTION_IDS_RESOURCE_PATH = /api/ReportData/whatever-this-happens-to-be
ACTION_ID_CUSTOM_FIELD_ID = 123
```

### Environment Variables

- `BASE_RESOURCE_URL` - Base URL of your Halo instance (include trailing slash)
- `CLIENT_ID` - OAuth2 client ID for API authentication
- `CLIENT_SECRET` - OAuth2 client secret for API authentication
- `ACTION_IDS_RESOURCE_PATH` - API path to the report that returns existing action IDs (e.g., `/api/ReportData/YourReportName`)
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
2. Fetch existing action IDs from the configured report
3. Process all CSV and Excel files in the `input/` directory
4. Skip actions that already exist
5. Import new actions with a 500ms delay between API calls
6. Generate a log file in the `log/` directory with a UTC timestamp

### Custom Input Directory

To process files from a different directory:

```bash
cargo run --release -- --input input/1
```

This is useful for running multiple instances in parallel on different input directories. You can organize files into subdirectories like `input/1/`, `input/2/`, etc., and run separate instances for each.

### Parse-Only Mode

To validate files without making API calls:

```bash
cargo run --release -- --only-parse
# or use short form
cargo run --release -- --op
```

This mode:
- Parses all files and validates data structure
- Shows which actions would be imported or skipped
- Does not make any API calls
- Uses reduced logging frequency (every 10,000 entries or 5 seconds)
- Shows success message if all actions parse successfully

### Batch Mode

Post multiple actions per API request (default is 1):

```bash
cargo run --release -- --batch 10
```

Batch mode groups actions into batches of the specified size before posting to the API. This significantly improves throughput by reducing the number of API calls and network overhead.

### Reverse Mode

Process files from bottom to top (useful for parallel execution):

```bash
cargo run --release -- --reverse
# or use short form
cargo run --release -- --rev
```

### Half Mode

Process only half the files (combine with `--reverse` for bottom half):

```bash
# Process top half
cargo run --release -- --half

# Process bottom half
cargo run --release -- --half --reverse
```

This is useful for running two instances in parallel - one processing the top half, one processing the bottom half.

### Combined Modes

You can combine flags:

```bash
# Parse-only mode in reverse
cargo run --release -- --only-parse --reverse

# Import bottom half in reverse
cargo run --release -- --half --reverse

# Process custom directory with half mode
cargo run --release -- --input input/1 --half

# Run multiple instances in parallel (different subdirectories with batch mode)
cargo run --release -- --input input/1 --batch 10 &
cargo run --release -- --input input/2 --batch 10 &
cargo run --release -- --input input/3 --batch 10 &
```

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

## Logging

Logs are written to both:
- Console (stdout)
- Log file: `log/YYYY-MM-DD_HH-MM-SS.log` (UTC timestamp with seconds)

All log entries include timestamps with seconds for precise tracking.

### Log Levels

- `trace` - Most verbose, includes all internal operations
- `debug` - Debug information and detailed flow
- `info` - General information, progress updates, summaries (default)
- `warn` - Warnings and non-critical errors (e.g., missing tickets)
- `error` - Errors that prevent processing

### Progress Updates

- **Import mode**: Updates every 100 entries or 1 minute
- **Parse-only mode**: Updates every 10,000 entries or 5 seconds

Progress logs include:
- Current row count and percentage complete
- Number imported and skipped
- Average time per row (based only on actual imports, not skips)
- Estimated time remaining (formatted as days/hours/minutes/seconds)

### Log Messages

**Skip Messages**: Consecutive skips are batched into single messages:
```
Skipped 1,234 entries (already exist)
```

**Success Messages**: Each successful import is logged. When using `--batch` mode with batch size > 1:
```
Success: imported batch of 10 actions
```
For batch size of 1 (default):
```
Success: imported action ID: 12345 (ticket ID: 67890)
```

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

## Error Handling

The application is designed to be resilient:
- Deserialization errors are logged and the row is skipped
- API errors are logged and processing continues
- File read errors are logged and the file is skipped
- Missing tickets are detected and future actions for them are automatically skipped
- Token expiration is handled automatically with refresh and retry
- 401 Unauthorized responses trigger automatic token refresh and retry
- All errors are collected and reported in the final summary

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
