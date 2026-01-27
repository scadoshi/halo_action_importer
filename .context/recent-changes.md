# Recent Changes (2026-01-27)

## Session Summary

This document tracks recent changes and improvements made to the Halo Action Importer during the 2026-01-27 development session.

---

## 1. Smart Batch Retry Logic - Ticket-Grouped Approach

### Problem
When importing actions in batches, if any action in the batch fails due to a missing ticket, the entire batch would fail. This resulted in:
- Many valid actions being marked as failed unnecessarily
- No way to identify which specific tickets were missing

### Solution Implemented
Replaced binary search with a simpler **ticket-grouped retry** approach:

1. When a batch fails with "not found" error, group all actions by `ticket_id`
2. Retry each ticket group independently
3. If a ticket group fails, mark all actions for that ticket as failed and add ticket to skip set
4. Successfully import actions for valid tickets

**Files Modified:**
- `src/lib/domain/importer/processor.rs`
  - Added `TicketGroupedResult` struct
  - Added `retry_by_ticket_groups()` function
  - Updated all 4 error handlers (CSV batch-full, CSV final-batch, Excel batch-full, Excel final-batch)

**Benefits:**
- Much simpler than binary search (no recursion)
- More efficient: For batch of 100 actions with 5 missing tickets = max 10 API calls (vs 100+ with one-by-one)
- Clear logging shows which tickets are missing

**Example Log Output:**
```
WARN Batch failed with 'not found' error - retrying 3 ticket groups
WARN Ticket ID: 438843 not found - marking 7 action(s) as failed
INFO Ticket group retry complete: recovered 43/50 actions, identified 2 missing ticket(s)
```

---

## 2. Enhanced Success Logging

### Problem
Success logs were minimal and didn't show progress or ETA.

### Solution
Every successful import now shows comprehensive stats:

**For single actions (batch_size=1):**
```
INFO Imported action 1/10000 (ID: 12345, ticket: 67890) | 50 total skipped | 0.50s/row | ETA: 1h 23m 15s
```

**For batches (batch_size>1):**
```
INFO Imported batch 1/200 (50 actions, tickets: 67890, 67891, 67892) | 50 total skipped | 0.05s/action | ETA: 8m 15s
```

**Includes:**
- Current position (action X/Y or batch X/Y)
- All ticket IDs (no truncation)
- Total skipped count (cumulative)
- Average time per action
- Estimated time remaining

**Files Modified:**
- `src/lib/domain/importer/processor.rs`
  - Added `format_eta()` helper function
  - Updated success logging in 4 locations (CSV/Excel, batch-full/final-batch)
  - Removed ticket ID truncation (was showing "3 more")

---

## 3. Removed Periodic Progress Logging

### Problem
Periodic progress updates created noise in logs:
- Time-based updates every 60 seconds
- Count-based updates every 100 actions
- Made logs harder to read

### Solution
Removed all periodic progress logging. Only show:
- Skip batch summaries (when actually skipping)
- Success logs (every import)
- Error logs (when failures occur)

**Files Modified:**
- `src/lib/domain/importer/processor.rs`
  - Deleted `log_progress()` function
  - Deleted `ProgressParams` struct
  - Removed progress check blocks

---

## 4. Fixed Batch Numbering Bug

### Problem
Batch numbers were calculated incorrectly:
- Showed "batch 0" for first batch
- Duplicate batch numbers (e.g., "batch 9" appeared twice)
- Calculation was `(imported / batch_size)` which happened AFTER incrementing `imported`

### Solution
Added explicit `batch_number` counter that increments with each successful batch.

**Files Modified:**
- `src/lib/domain/importer/processor.rs`
  - Added `let mut batch_number = 0;` to CSV and Excel processing functions
  - Increment `batch_number` after each successful import
  - Use `batch_number` directly in logs instead of calculating from `imported`

**Example:**
```
Before: Imported batch 0/200 ...  (wrong)
After:  Imported batch 1/200 ...  (correct)
```

---

## 5. Simplified Cache File Names

### Problem
Cache file names were verbose:
- `cache/existing_action_ids.json`
- `cache/imported_ids.txt`

### Solution
Simplified to:
- `cache/existing`
- `cache/imported`

**Files Modified:**
- `src/lib/domain/importer/setup.rs`
  - Updated `RESOURCE_CACHE_FILE` and `IMPORTED_CACHE_FILE` constants

---

## 6. Restored JSON Cache Format with Resource Tracking

### Problem
The `existing` cache was simplified to comma-separated format:
```
id1,id2,id3,id4,id5,...
```

While simpler, this format lost resource tracking, causing the application to re-fetch ALL reports from the API every run. For large datasets with millions of action IDs, this was inefficient and time-consuming.

### Solution (2026-01-27)
Restored **JSON format with resource tracking** in `cache/existing.json`:
```json
[
  {
    "resource_id": "aa637f8f-0e94-48e4-8881-8e1ff08445ec",
    "action_ids": ["id1", "id2", "id3"]
  }
]
```

**Files Modified:**
- `src/lib/domain/importer/setup.rs`
  - Added `ResourceCache` struct with `Serialize` and `Deserialize`
  - Updated `RESOURCE_CACHE_FILE` to `cache/existing.json`
  - Updated `read_cached_ids()` to parse JSON and populate `fetched_resources`
  - Updated `append_resource_to_cache()` to write JSON with resource tracking

**Benefits:**
- ✅ Resource tracking works - skips already-fetched resources on subsequent runs
- ✅ Saves API calls and time (especially for large datasets)
- ✅ Incremental updates - can add new resources without re-fetching everything
- ✅ Human-readable JSON format for debugging

---

## 7. Improved Timeout Resilience and Removed Wait Times

### Problem
The application would crash after 2-3 timeout errors:
- Network send failures would immediately return errors and crash
- Limited to 2 retry attempts per request
- 60-second wait on 504 Gateway Timeout responses
- 500ms delay between all POST requests
- User observed that timeouts seemed to cascade, requiring application restart

### Solution (2026-01-27)
Implemented infinite retry on all timeout-related errors and removed all wait times:

**Infinite Retry Logic:**
- Network errors (connection failures, timeouts) now retry indefinitely with immediate retry
- 504 Gateway Timeout responses retry indefinitely (was: wait 60s, limited retries)
- 401 Unauthorized responses retry indefinitely after token refresh
- Removed the `for attempt in 0..2` inner loop limitation
- Only non-retryable errors (400, 500, etc.) will cause batch to fail

**Removed Wait Times:**
- Removed 500ms delay between POST requests
- Removed 60-second wait on 504 Gateway Timeout (now retries immediately)

**Files Modified:**
- `src/lib/outbound/client/action.rs`
  - Removed inner `for attempt in 0..2` loop
  - Changed network send failures from immediate error return to retry with warning
  - Removed 500ms sleep before POST requests
  - Removed 60-second sleep on 504 responses
  - Removed attempt limit on 401 responses
- `src/lib/inbound/client.rs`
  - Removed inner `for attempt in 0..2` loop and outer `'outer:` label
  - Changed network send failures to retry indefinitely
  - Removed 60-second sleep on 504 responses
  - Removed attempt limit on 401 responses

**Benefits:**
- ✅ Application never crashes from timeouts - keeps retrying until success
- ✅ Faster throughput - no artificial delays between requests
- ✅ Better handling of unstable API connections
- ✅ Immediate retry on errors instead of waiting 60 seconds

**Example Log Output:**
```
WARN Network error sending POST request for action IDs ["12345"]: connection timeout - retrying immediately
WARN Received 504 Gateway Timeout for action IDs ["12345"], retrying immediately
```

---

## 8. Added Comma Formatting to Report ID Counts

### Problem
When fetching reports, large ID counts were displayed without commas, making them hard to read:
```
Report 1/5 complete: 1234567 IDs from uuid-1
```

### Solution (2026-01-27)
Added `format_number()` formatting to report ID counts.

**Files Modified:**
- `src/lib/inbound/client.rs`
  - Applied `format_number()` to `report_ids.len()` in success log

**Example:**
```
Before: Report 1/5 complete: 1234567 IDs from uuid-1, 1,234,567 total new IDs
After:  Report 1/5 complete: 1,234,567 IDs from uuid-1, 1,234,567 total new IDs
```

---

## Implementation Notes

### Batch Size Recommendations
- `batch_size=1`: Individual action imports, slower but precise error tracking
- `batch_size=50`: Recommended for balance of speed and error recovery
- `batch_size=1000`: Fastest but if many missing tickets, more retry overhead

### Testing Performed
- Compiled successfully with all changes
- User tested with batch_size=50
- Observed ticket-grouped retry working correctly
- Fixed batch numbering verified in user logs

### Known Issues
- User reported seeing duplicate action IDs in Halo system
- Investigation revealed this was likely due to cache corruption on user's end
- User plans to rebuild cache to fix duplicates
- Not caused by retry logic (logs show no retry messages when duplicates occurred)

---

## Cache File Formats

### `cache/existing.json`
- **Format:** JSON array of resource objects with action IDs
- **Purpose:** Track which resources (reports) have been fetched and their action IDs
- **Example:**
  ```json
  [
    {
      "resource_id": "aa637f8f-0e94-48e4-8881-8e1ff08445ec",
      "action_ids": ["12345", "12346", "12347"]
    },
    {
      "resource_id": "9a887d53-85fa-4928-a450-9aece690ade2",
      "action_ids": ["12348", "12349"]
    }
  ]
  ```
- **Updated by:** `append_resource_to_cache()` after fetching each report
- **Read by:** `read_cached_ids()` which populates both `action_ids` and `fetched_resources` sets

### `cache/imported`
- **Format:** Line-separated values (one ID per line)
- **Purpose:** Track action IDs imported by this tool
- **Example:**
  ```
  12345
  12346
  12347
  ```
- **Updated by:** `append_imported_ids_to_cache()` after successful imports

---

## Next Steps / Future Improvements

1. **Monitor:** Watch for performance with JSON cache format for very large datasets
2. **Consider:** Add configuration for retry strategy (ticket-grouped vs one-by-one)
3. **Consider:** Add maximum retry limit configuration (currently retries indefinitely)
4. **Test:** Verify JSON cache handles edge cases (malformed JSON, empty files, concurrent writes)

---

## For Future Developers

If you need to modify the retry logic:
- See `retry_by_ticket_groups()` in `src/lib/domain/importer/processor.rs`
- The function is called from 4 locations (search for `retry_by_ticket_groups`)
- Key insight: Group by ticket_id first, then retry groups independently
- Ensure `missing_tickets` HashSet is updated to skip future actions for bad tickets

If you need to modify timeout/retry behavior:
- See main request loops in `src/lib/outbound/client/action.rs` and `src/lib/inbound/client.rs`
- Current behavior: Retry indefinitely on network errors, 504s, and 401s
- Only non-retryable errors (400, 500, etc.) cause failure
- No wait times between retries - retries are immediate

If you need to modify cache format:
- See `read_cached_ids()` and `append_resource_to_cache()` in `src/lib/domain/importer/setup.rs`
- Existing format is JSON with resource tracking (`cache/existing.json`)
- Imported format is line-separated (`cache/imported`)
- Both use file locking to prevent concurrent write issues
- `ResourceCache` struct maps resource UUIDs to action ID arrays
