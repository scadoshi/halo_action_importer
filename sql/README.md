# Report queries

These queries are only for `--sync-from-halo`. A normal run reads the local cache and never asks Halo for its IDs. Sync is for when that cache is gone or can't be trusted.

Each query returns the action IDs already imported, read from the `cfactionid` custom field. If `ACTION_ID_CUSTOM_FIELD` names a different field, change it in the query too.

- `simple.sql` returns every ID in one comma-separated string.
- `single_group.sql` returns one slice of the IDs. Set `@num_groups` to the number of reports and `@target_group` to this report's slice, then make one report per slice.
- `groups.sql` returns all the slices in one result, to check how they split.

Keep the `--dynamic` line at the top. Halo needs it to allow variables and CTEs.

Put the report UUIDs in `.env`, comma separated:

```env
ACTION_IDS_RESOURCE_PATHS = uuid-1,uuid-2
```

Leave one out and its IDs get imported again.

## Large imports

Halo reports start timing out once they serve a few million IDs. Splitting the IDs across more reports helps a little, but a dozen reports gets unwieldy fast. Past a certain size the API can't hand back the imported IDs at all, and the only other place to get them is a query against Halo's SQL Server database, which needs that access.

So for a really large import, run a single importer and guard its `cache/` directory. Back it up as you go. Most imports are nowhere near that size.
