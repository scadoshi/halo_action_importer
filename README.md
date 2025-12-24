# Halo Action Importer

A Rust application for bulk importing actions into Halo. Processes CSV and Excel files, skips existing actions based on unique identifiers, and provides comprehensive logging and performance statistics.

## Features

- Bulk import actions from CSV and Excel files
- Automatic duplicate detection using unique action identifiers
- Incremental processing to handle large files efficiently
- Comprehensive logging with configurable log levels
- Performance statistics (runtime, entries per minute, time per entry)
- Parse-only mode for validation without API calls
- Progress tracking with configurable update frequencies
- Error handling that continues processing on failures

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

### Parse-Only Mode

To validate files without making API calls:

```bash
cargo run --release -- --only-parse
```

This mode:
- Parses all files and validates data structure
- Shows which actions would be imported or skipped
- Does not make any API calls
- Uses reduced logging frequency (every 10,000 entries or 5 seconds)
- Shows success message if all actions parse successfully

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
- Log file: `log/importer_YYYY-MM-DD_HH-MM-SS.log` (UTC timestamp)

### Log Levels

- `trace` - Most verbose, includes all internal operations
- `debug` - Debug information and detailed flow
- `info` - General information, progress updates, summaries (default)
- `warn` - Warnings and non-critical errors
- `error` - Errors that prevent processing

### Progress Updates

- **Import mode**: Updates every 100 entries or 1 minute
- **Parse-only mode**: Updates every 10,000 entries or 5 seconds

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
- All errors are collected and reported in the final summary

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
