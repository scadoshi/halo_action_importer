# ID Initializer (idinit)

Quick and dirty tool to add `CFactionId` to CSV and Excel files that are missing them.

## What it does

1. Scans `../input/still_to_do` for all CSV and Excel files
2. Finds the maximum `CFactionId` across all files
3. For each file:
   - If the file has no `CFactionId` column, adds it as the first column
   - Fills in any missing or invalid IDs with sequential values starting from max+1
4. Overwrites files in place

## Usage

```bash
cd idinit
cargo run --release
```

That's it. No command line arguments, no configuration.

## Notes

- Hard-coded to look in `../input/still_to_do`
- Supports both CSV (`.csv`) and Excel (`.xlsx`, `.xls`) files
- Recognizes both `CFactionId` and `actionId` as ID columns (case-insensitive)
- IDs must be numeric (u64)
- Files are modified in place - make backups if needed!

## Why this exists

Temporary utility to prepare input files that don't have unique identifiers before running the main importer. May be deleted later.
