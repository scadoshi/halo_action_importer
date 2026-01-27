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

## 6. Changed 'existing' Cache Format

### Problem
The `existing` cache file used JSON format with resource tracking:
```json
[
  {
    "resource_id": "report1",
    "action_ids": ["id1", "id2", "id3"]
  }
]
```

This was complex and harder to read/edit manually.

### Solution
Changed to simple **comma-separated on single line**:
```
id1,id2,id3,id4,id5,...
```

**Files Modified:**
- `src/lib/domain/importer/setup.rs`
  - Updated `read_cached_ids()` to parse comma-separated format
  - Updated `append_resource_to_cache()` to write comma-separated format
  - Removed `ResourceCache` struct (no longer needed)
  - Removed unused `Serialize` and `Deserialize` imports

**Trade-offs:**
- ✅ Simpler format, easier to read/edit
- ✅ One line instead of multi-line JSON
- ⚠️ Lost resource tracking (app will re-fetch all reports each run)
- User indicated this is acceptable: "don't worry about existing cache"

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

### `cache/existing`
- **Format:** Comma-separated values on single line
- **Purpose:** Track action IDs already existing in Halo (from reports)
- **Example:** `12345,12346,12347,12348,...`
- **Updated by:** `append_resource_to_cache()` after fetching reports

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

1. **Consider:** Add back resource tracking in separate file if re-fetching reports becomes too slow
2. **Consider:** Add configuration for retry strategy (ticket-grouped vs one-by-one)
3. **Monitor:** Watch for any performance issues with comma-separated format for very large caches
4. **Test:** Verify cache format handles edge cases (empty files, whitespace, etc.)

---

## For Future Developers

If you need to modify the retry logic:
- See `retry_by_ticket_groups()` in `src/lib/domain/importer/processor.rs`
- The function is called from 4 locations (search for `retry_by_ticket_groups`)
- Key insight: Group by ticket_id first, then retry groups independently
- Ensure `missing_tickets` HashSet is updated to skip future actions for bad tickets

If you need to modify cache format:
- See `read_cached_ids()` and `append_resource_to_cache()` in `src/lib/domain/importer/setup.rs`
- Existing format is comma-separated on one line
- Imported format is line-separated
- Both use file locking to prevent concurrent write issues
