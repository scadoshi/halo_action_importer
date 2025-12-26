# SQL Queries for Halo Action Importer

This folder contains SQL queries to run on the Halo instance to gather existing action IDs for skip detection.

## Files

### `simple.sql`
Original query that returns all existing action IDs as a single comma-separated string in one row. Works for smaller datasets but times out with ~3 million IDs.

### `groups.sql`
Splits action IDs into multiple rows/groups. Returns N rows, each containing a comma-separated subset of action IDs. Useful for testing group distribution. Configurable via `@num_groups` variable.

### `single_group.sql`
Returns only a single group's IDs. Use this to create multiple separate Halo reports, each targeting a different group (1-10). Set `@num_groups` to total groups and `@target_group` to which group this report should return.

## Usage

### For Production (Multiple Reports)
1. Create 10 separate reports in Halo, each using `single_group.sql`
2. Set `@num_groups = 10` and `@target_group = 1` through `10` for each report
3. Copy all 10 report URLs as comma-separated list in `.env`:
   ```
   ACTION_IDS_RESOURCE_PATH=/api/ReportData/uuid-1,/api/ReportData/uuid-2,...
   ```
4. **CRITICAL:** Include ALL report URLs or the application will import duplicate actions

### For Testing
Run `groups.sql` to see all groups in one query. Adjust `@num_groups` to test different distributions.

**Note:** The `--dynamic` declaration at the top is required by Halo's web application to use CTEs and variables.

