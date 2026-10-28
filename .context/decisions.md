# Decisions

The README covers how to run the importer. This is the reasoning behind the parts that look odd from the code alone.

The local cache is the source of truth for what Halo already has. Reports that list imported IDs time out once they serve a few million, so a normal run never asks for them; `--sync-from-halo` is the recovery path. A run with an empty cache refuses unless it syncs or passes `--fresh`, because an empty cache by mistake means posting every action twice.

Every posted ID is appended to `cache/imported` as soon as its post succeeds, in one synced write, so a crash loses at most the batch in flight. `existing.json` is replaced through a temp file and a rename.

A post is never sent twice. Halo may have stored a batch even when the reply was a 504 or never came, so the batch goes to `retry.csv` and a later `--sync-from-halo` sorts out what landed. Report fetches are reads, so they do retry: three tries, a minute apart.

Only a 404 means the ticket is missing. The response body is not read for words like "not found", since a 400 for a missing user says that too. A batch that gets a 404 is posted again one ticket at a time so the other tickets' actions still go in, and the missing ticket's later actions are skipped for the rest of the run.

Action IDs are numbers everywhere: `0042` in a file and `42` in the cache are the same action. Action dates in the files are Arizona time (UTC-7, no daylight saving) and are sent as UTC.

Tests are the only proof the importer works; nobody runs it by hand against a real instance. HTTP behavior is tested against `src/lib/test_server.rs`, a local server that answers each request with the next scripted reply, and Excel input against small workbooks written by `src/lib/test_xlsx.rs`.
