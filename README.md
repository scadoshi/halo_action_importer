# Halo Action Importer

Bulk-imports actions into Halo from CSV and Excel files. Each action carries its own ID in a custom field. The importer records every ID it posts in a local cache and skips those IDs next time, so a run can be stopped and started again without creating duplicates.

## Requirements

- A Halo instance with API access, and an API application (client ID and secret).
- For `--sync-from-halo` only: one or more Halo reports that return the action IDs already imported. See `sql/README.md`.
- A custom field on actions holding the unique ID.
- A Rust toolchain.

## Configuration

A `.env` in the project root:

```env
BASE_RESOURCE_URL = https://example.haloitsm.com/
CLIENT_ID = client-id
CLIENT_SECRET = client-secret
ACTION_IDS_RESOURCE_PATHS = aa637f8f-0e94-48e4-8881-8e1ff08445ec,9a887d53-85fa-4928-a450-9aece690ade2
ACTION_ID_CUSTOM_FIELD = cfactionid
LOG_LEVEL = info
```

`BASE_RESOURCE_URL` keeps its trailing slash; the token URL is derived from it. `ACTION_IDS_RESOURCE_PATHS` is the report UUIDs that `--sync-from-halo` reads, comma separated; the importer builds the URLs. `ACTION_ID_CUSTOM_FIELD` is the name of the action custom field that holds the ID, such as `cfactionid`; each action is posted with its ID under that name. `LOG_LEVEL` is one of trace, debug, info, warn, error and defaults to info. A missing or malformed value stops the run and names the variable.

## Running

Put the files in `input/` and run:

```bash
cargo run --release
```

The importer authenticates, reads the IDs already imported from `cache/`, parses every `.csv`, `.xlsx` and `.xls` in the directory, skips those IDs, posts the rest, and writes a log directory for the run. It does not ask Halo what it already has unless you pass `--sync-from-halo`.

`cargo run --release -- --help` lists the options. Each has a long name and two shorter spellings; an unknown flag or a bad value stops the run rather than being ignored.

| Option | Also | Does |
|---|---|---|
| `--input-path <dir>` | `--input`, `--ip` | Directory to read instead of `input/` |
| `--batch-size <n>` | `--batch`, `--bs` | Actions per request, at least 1 (the default) |
| `--only-parse-inputs` | `--only-parse`, `--op` | Authenticate, parse and report, post nothing |
| `--sync-from-halo` | `--sync`, `--sfh` | Fetch the IDs from the reports and merge them into `cache/` first |

Parse-only is the way to check the files before a real run. Batching is the way to make a large run fast; a batch is posted in one request.

Sync when the cache can't be trusted: it was lost, you're on a new machine, or a run hit 504s or timeouts on posts that Halo may have stored anyway. A sync fetches every configured report again, even ones already in the cache.

To run several imports at once, split the files into directories and start one process per directory:

```bash
cargo run --release -- --ip input/1 --bs 10 &
cargo run --release -- --ip input/2 --bs 10 &
```

The processes append to the same cache files under a lock, but each reads the cache only when it starts, so an ID that appears in two directories can be posted twice.

## Input files

Columns, by header name:

| Column | Also | Holds |
|---|---|---|
| `CFactionId` | `actionId` | The action's unique ID |
| `requestId` | `request_id`, `ticket_id` | The ticket the action belongs to |
| `actionWho` | | Who performed it |
| `note` | | The note text |
| `actionDate` | | ISO 8601, or an Excel serial date; read as Arizona time (UTC-7) and sent as UTC |
| `outcome` | | Optional; defaults to `Imported Note` |

Other columns are ignored.

## Cache

The cache is how the importer knows what Halo already has, so back it up during a large import. `cache/imported` gets one action ID per line, written after each successful post. `cache/existing.json` holds the IDs each report returned on the last sync. An ID in either file is skipped, and so is an ID already posted earlier in the same run.

With an empty cache, every action in the files is posted. If the instance already has imported actions, run with `--sync-from-halo` first.

A `cache/existing.json` that is not valid JSON stops the run and names the file. It is written to a temporary file and renamed into place, so an interrupted write leaves the previous version. A last line in `cache/imported` that a crash cut short is ignored.

## Output

Each run writes `log/YYYY-MM-DD_HH-MM-SS/`:

- `full.log`, everything that was printed, with RFC 3339 timestamps
- `retry.csv`, the failed actions in the input column layout plus an `error_type`, when any failed. It can be dropped straight back into `input/`
- `summary.json`, counts, timings and failures by error type

Progress is printed on every successful post rather than on a timer: position, IDs, cumulative skips, average time per action and the time remaining. Consecutive skips collapse into one line.

## Failures

A row that does not deserialize is logged and skipped; a file that cannot be read is logged and skipped; the run continues. After the summary, the process exits 1 when any action failed or any file could not be read, and 0 otherwise. A request gives up after five minutes. A report fetch that hits a network error, a timeout or a 504 is tried again a minute later, three tries in all. A post is sent once: Halo may have stored the actions even when the reply is a 504 or never comes, so the batch goes to `retry.csv` instead of being sent again. A 401 gets a new token and one more try; tokens are also refreshed thirty seconds before they expire.

A post Halo answers with a 404 means the ticket does not exist: it is logged once, and every later action for that ticket is skipped. When a batch gets a 404, it is regrouped by ticket and each group is posted on its own, so one missing ticket costs only its own actions. Any other rejection fails its batch, whatever the reply says.

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
