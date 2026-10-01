# Halo Action Importer

Bulk-imports actions into Halo from CSV and Excel files. Each action carries its own ID in a custom field, so the importer can tell what is already in Halo, skip it, and be rerun or resumed without creating duplicates.

## Requirements

- A Halo instance with API access, and an API application (client ID and secret).
- One or more Halo reports that return the action IDs already imported. Templates are in `sql/`. Past about three million IDs, split them across several reports or the report times out.
- A custom field on actions holding the unique ID.
- A Rust toolchain.

## Configuration

A `.env` in the project root:

```env
BASE_RESOURCE_URL = https://example.haloitsm.com/
CLIENT_ID = client-id
CLIENT_SECRET = client-secret
ACTION_IDS_RESOURCE_PATHS = aa637f8f-0e94-48e4-8881-8e1ff08445ec,9a887d53-85fa-4928-a450-9aece690ade2
ACTION_ID_CUSTOM_FIELD_ID = 123
LOG_LEVEL = info
```

`BASE_RESOURCE_URL` keeps its trailing slash; the token URL is derived from it. `ACTION_IDS_RESOURCE_PATHS` is the report UUIDs, comma separated; the importer builds the URLs. `LOG_LEVEL` is one of trace, debug, info, warn, error and defaults to info. A missing or malformed value stops the run and names the variable.

## Running

Put the files in `input/` and run:

```bash
cargo run --release
```

The importer authenticates, fetches the existing action IDs from the reports (or the cache), parses every `.csv`, `.xlsx` and `.xls` in the directory, skips what Halo already has, posts the rest, and writes a log directory for the run.

`cargo run --release -- --help` lists the options. Each has a long name and two shorter spellings; an unknown flag or a bad value stops the run rather than being ignored.

| Option | Also | Does |
|---|---|---|
| `--input-path <dir>` | `--input`, `--ip` | Directory to read instead of `input/` |
| `--batch-size <n>` | `--batch`, `--bs` | Actions per request, at least 1 (the default) |
| `--only-parse-inputs` | `--only-parse`, `--op` | Authenticate, fetch the IDs, parse and report, post nothing |
| `--only-use-cache` | `--only-cache`, `--oc` | Skip the reports and trust `cache/` for existing IDs |

Parse-only is the way to check the reports and the files before a real run. Batching is the way to make a large run fast; a batch is posted in one request.

To run several imports at once, split the files into directories and start one process per directory:

```bash
cargo run --release -- --ip input/1 --bs 10 &
cargo run --release -- --ip input/2 --bs 10 &
```

The processes share the cache files under a lock, so they do not re-import each other's actions.

## Input files

Columns, by header name:

| Column | Also | Holds |
|---|---|---|
| `CFactionId` | `actionId` | The action's unique ID |
| `requestId` | `ticket_id` | The ticket the action belongs to |
| `actionWho` | | Who performed it |
| `note` | | The note text |
| `actionDate` | | ISO 8601, or an Excel serial date; read as Arizona time (UTC-7) and sent as UTC |
| `outcome` | | Optional; defaults to `Imported Note` |

Other columns are ignored.

## Cache

`cache/existing.json` records, per report UUID, the action IDs it returned, so a rerun does not fetch a report it has already read. `cache/imported` is one action ID per line, appended as imports succeed. Together they let an interrupted run resume where it stopped. Delete `cache/` to start from the reports again.

## Output

Each run writes `log/YYYY-MM-DD_HH-MM-SS/`:

- `full.log`, everything that was printed, with RFC 3339 timestamps
- `retry.csv`, the failed actions in the input column layout plus an `error_type`, when any failed. It can be dropped straight back into `input/`
- `summary.json`, counts, timings and failures by error type

Progress is printed on every successful post rather than on a timer: position, IDs, cumulative skips, average time per action and the time remaining. Consecutive skips collapse into one line.

## Failures

A row that does not deserialize is logged and skipped; a file that cannot be read is logged and skipped; the run continues. Network errors and 504s retry until they succeed. A 401 refreshes the token and retries once; tokens are refreshed thirty seconds before they expire regardless.

A ticket that Halo reports as not found is logged once, and every later action for it is skipped. When a whole batch fails that way, the batch is regrouped by ticket and each group is posted on its own, so one missing ticket costs its own actions and not the batch.

## Layout

```
src/
├── bin/main.rs              entry point
└── lib/
    ├── cli.rs               command line options
    ├── config.rs            configuration from the environment
    ├── domain/
    │   ├── importer/
    │   │   ├── setup.rs     logging, auth, cache, file discovery
    │   │   ├── processor.rs CSV and Excel processing, batching, retry
    │   │   ├── format.rs    numbers and durations as the log prints them
    │   │   └── summary.rs   the end-of-run summary
    │   └── models/          the action as Halo takes it
    ├── inbound/
    │   ├── client.rs        report client for existing IDs
    │   └── file/            CSV and Excel readers
    └── outbound/client/
        ├── action.rs        posting actions
        └── auth/            OAuth2 tokens
```

## Building

```bash
cargo build --release
cargo clippy --all-targets -- -D warnings
cargo test
cargo test -- --ignored   # posts one real action to the instance in .env
```

## License

MIT, see `LICENSE`.
