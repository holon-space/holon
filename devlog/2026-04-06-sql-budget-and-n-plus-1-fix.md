# SQL Budget Formulas + N+1 Parent Chain Fix

## What changed

### 1. Rewritten SQL budget formulas (`transition_budgets.rs`)

Replaced imprecise formulas with empirically-derived constants:
- `REACTIVE_BASE = 5` (reactive engine reads)
- `JOURNAL_READS = 4` (operation journal DML)
- `NAV_DML_READS = 5` (navigation DML)
- `CACHE_EVENT_READS = 3` (cache subscriber per CDC event)
- `READS_PER_WATCH = 2` (per user watch)

6 transition types now have **zero tolerance**: SwitchView, RemoveWatch, SetupWatch, NavigateFocus, NavigateHome, NavigateBack.

### 2. `HOLON_PERF_BUDGET` env var

Set `HOLON_PERF_BUDGET=0` to downgrade inv13 violations to warnings. Default: on.

### 3. N+1 fix: recursive CTE for `find_document_uri`

Replaced O(depth) parent chain walk (one SELECT per ancestor) with a single recursive CTE query. Located in `sql_operation_provider.rs`.

**Impact:**
- Update reads: 15-20 → 11 (external) / 16 (UI, exact match)
- Create reads: 8-28 → 12-21 (much more stable)
- CDC tolerance: `4 + blocks` → `6 + blocks/4` (4x tighter)

### Key finding

The handoff hypothesized internal watches (region, all-blocks, structural) contribute counted reads. Investigation showed they use `subscribe_sql → matview CDC broadcast`, which does NOT generate "query" spans. Only user watches from SetupWatch contribute. The real excess came from org sync CDC cascades driving parent chain walks.

### 4. Journal batching: INSERT RETURNING (`operation_log.rs`)

Replaced 4 separate queries per undo journal log with 2:
- `INSERT INTO operation (...) RETURNING id` replaces INSERT + SELECT last_insert_rowid()
- `SELECT COUNT(*)` for trim amortized to every 10th operation

**Impact:** Update reads dropped from 13 to 11 (in PBT, which includes test overhead).

### 5. Production vs test SQL audit

Discovered that 6 of the 13 reads per Update are **test-only**:
- 3× focus_roots per region (test region watches)
- 1× current_focus (invariant check)
- 1× properties IS NOT NULL (invariant check)
- 1× name IS NULL (wait_for_block_count polling)

**Production Update cost: 5 reads + 3 writes = 8 SQL statements** (down from 16 at session start).

## Remaining follow-ups

- Cache doc URI mapping (save 1 production read per mutation)
- Merge event status UPDATE into INSERT (save 1 production write per mutation)
- These would bring production Update to 4 reads + 2 writes = 6 statements.
