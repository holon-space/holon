# Holon Code Review - Cleanup Tasks

Comprehensive code review conducted 2026-02-09 by 9 parallel review agents examining every crate and frontend.

---

## Cross-Cutting Themes

### Theme 1: Defensive Programming Violations

Multiple crates use `.unwrap_or(default)` and `.ok()?` where `.expect("reason")` should be used per coding guidelines. Found in:

- `crates/holon/src/core/operation_log.rs` - `.unwrap_or(0)` hides schema bugs in COUNT query
- `crates/holon/src/core/queryable_cache.rs` - `.unwrap_or("id")` silently assumes primary key name
- `crates/holon-api/src/entity.rs` - `DynamicEntity` accepts invalid fields without validation
- `crates/holon-todoist/` - API failures logged but not properly escalated

**Action**: Systematic audit replacing defensive `.unwrap_or()` with `.expect()` for programmer errors. Reserve `Result` propagation only for genuinely expected failure modes (network errors, user input).

### Theme 2: Missing Test Coverage

Happy paths are well tested, hard cases are not.

| Crate | Happy Path | Edge Cases | Concurrency | Score |
|-------|-----------|------------|-------------|-------|
| holon (main) | 8/10 | 5/10 | 3/10 | 5/10 |
| holon-core | fractional_index, operation_log | No tests for BlockOperations (500+ lines) | None | 3/10 |
| holon-api | Good value/entity tests | Missing Reference variant tests | N/A | 7/10 |
| holon-macros | Preconditions tested | No tests for #[affects], #[triggered_by] | N/A | 4/10 |
| holon-prql-render | Excellent coverage | Good edge cases | N/A | 9/10 |
| holon-orgmode | 1 unit test total | None | None | 1/10 |
| holon-todoist | PBT tests exist | Missing inverse operation tests | None | 5/10 |
| frontends | Zero tests | None | None | 0/10 |
| integration tests | Good PBT infrastructure | Weak convergence assertions, no CDC verification | Partial | 5/10 |

### Theme 3: Architecture Drift

| Area | ARCHITECTURE.md Says | Code Does |
|------|---------------------|-----------|
| EventBus integration | QueryableCache uses EventBus | Uses direct broadcast channels |
| holon-core contents | Trait definitions only | Contains concrete BlockOperations implementations |
| Lens/Predicate traits | Listed as core abstractions | Don't exist at all |
| OperationProvider location | In holon-core | Exists as OperationRegistry (different API) |
| Operation return types | `Option<Operation>` | Migrated to `OperationResult` with `FieldDelta` |

---

## Issues by Crate

### crates/holon/ (Main Orchestration)

- [x] **P0** ~~Remove dead `_cdc_conn` field in `QueryableCache`~~ DONE: Removed field and constructor initialization. Verified it was always None (only set in TODO.md plan, never in code)
- [x] **P1** ~~Extract duplicated `value_to_sql()`~~ DONE: Created `storage/sql_utils.rs` with shared `value_to_sql_literal()`. Updated 4 call sites: `sql_operation_provider.rs`, `backend_engine.rs`, `turso.rs`, `e2e_backend_engine_test.rs`. Also fixed Array/Object handling (previously errored, now serializes as JSON)
- [x] **P1** ~~Extract duplicated entity short name derivation logic~~ DONE: Made `entity_short_name` an explicit parameter on `SqlOperationProvider::new()` across all 3 variants. Removed the `strip_prefix("test_")/trim_end_matches('s')` heuristic
- [x] **P1** ~~Replace defensive `.unwrap_or()` with `.expect()`~~ DONE: Replaced in `operation_log.rs` COUNT query, 5x primary key lookups in `queryable_cache.rs`, and 1x entity ID extraction. Left legitimate Option→Null conversions unchanged
- [x] **P1** ~~Add concurrent DDL/DML race condition tests (known bug class, untested)~~ DONE: Added 6 tests in turso_tests.rs: 20 concurrent inserts serialized by actor, DML before table created (error), DDL→DML sequence, concurrent DDL (create index) + DML (inserts), 50 concurrent updates on same row (counter=50), materialized view consistency after 20 concurrent writes
- [x] **P1** ~~Strengthen E2E test assertions - verify data correctness, not just change types~~ DONE: Enhanced CDC UI model check to verify all fields (content, content_type, source_language, source_name, parent_id with document URI normalization) not just id+content. Added per-mutation spot-check that verifies mutated block's content and content_type in DB immediately after each mutation
- ~~**P2** Simplify actor lifetime~~ INVALID: `_backend_keepalive` prevents real "Actor channel closed" bug when DI container drops before BackendEngine is used
- [x] **P2** ~~Split `di/mod.rs` (935 lines)~~ DONE: Split into `di/registration.rs`, `di/runtime.rs`, `di/lifecycle.rs` with re-exports in `mod.rs`
- [x] **P2** ~~Add cycle detection edge case tests for `LoroBackend::move_block`~~ DONE: Added 5 tests (self-cycle, direct child, deep descendant, valid move, nonexistent parent). Also fixed bug in `is_ancestor` that crashed on document URI parents
- [x] **P2** ~~Add undo/redo tests~~ DONE: Added 9 unit tests for UndoStack (empty stack, push/undo, undo/redo, redo cleared on new op, trim at max size, multiple undo/redo cycles, display names, update_redo_top, update_undo_top)
- [x] **P3** ~~Resolve EventBus vs broadcast channel architecture drift~~ DONE: ARCHITECTURE.md already documents the broadcast channel pattern as the current architecture (line 219) with EventBus as a planned future pattern (line 291). No change needed

### crates/holon-core/

- ~~**P2** Extract BlockOperations concrete implementations from traits.rs~~ INVALID: Default trait method implementations are part of trait definitions in Rust. The 500+ lines in BlockOperations are default methods that use only abstract trait methods (get_by_id, set_field, create, etc.). All 4 implementors (LoroBlockOperations, TodoistTaskOperations, TodoistTaskFake, MemStore) use `impl BlockOperations<T> for X {}` — moving these would require duplicating the logic. This is correct use of Rust trait defaults
- ~~**P2** Move `BlockEntity`/`TaskEntity` impl~~ INVALID: Would create circular dependency (holon-core → holon-api → holon-core). Orphan rule requires these impls stay in holon-core
- [x] **P2** ~~Split fat `BlockDataSourceHelpers` trait~~ DONE: Split into `BlockQueryHelpers` (5 read-only query methods, supertrait: DataSource) and `BlockMaintenanceHelpers` (2 mutating methods, supertrait: CrudOperations+DataSource). `BlockDataSourceHelpers` kept as backward-compatible alias combining both
- [x] **P2** ~~Remove blanket trait impls that auto-provide BlockOperations without explicit opt-in~~ DONE: Removed blanket impls from traits.rs, added explicit `impl BlockDataSourceHelpers` and `impl BlockOperations` on LoroBlockOperations, TodoistTaskOperations, and TodoistTaskFake
- [x] **P2** ~~Add tests for BlockOperations default implementations~~ DONE: Added 15 tests in `block_operations_tests.rs` using in-memory MemStore: move_block (beginning, after specific), indent, outdent (success + root fail), move_up/down (success + edge cases), split_block (middle, start, invalid position), inverse operations, descendant depth updates
- [x] **P3** ~~Add Lens and Predicate traits or update architecture docs~~ DONE: Removed Lens/Predicate from ARCHITECTURE.md. These were aspirational traits that were never implemented. PRQL-based queries replaced the need for type-safe query predicates
- [x] **P3** ~~Reconcile `OperationRegistry` with `OperationProvider` in architecture~~ DONE: Updated ARCHITECTURE.md to show `OperationRegistry` trait (the actual implementation) instead of `OperationProvider`. Updated trait signatures, field names, and return types to match current code
- [x] **P3** ~~Document `FieldDelta` vs CDC relationship~~ DONE: Added explanation to ARCHITECTURE.md under Operation Discovery: FieldDelta captures individual field changes at the operation level, CDC captures row-level changes at the database level — both exist because operations may affect multiple rows

### crates/holon-api/

- [x] **P0** ~~Remove `Value::Reference` variant~~ DONE: Removed variant from enum. Updated 12 files: all match arms either merged with String arm or removed. Updated frb_generated.rs to decode variant 6 as String for backward compat
- [x] **P2** ~~Document or fix `Schema::from_table_name()`~~ DONE: Added assertion in `to_create_table_sql()` that panics on empty fields with clear guidance
- ~~**P3** Add typed accessor extension traits for common StorageEntity fields~~ INVALID: Block already has direct `pub id: String` and `pub parent_id: String` fields. The `.get("id")` calls (78 occurrences) are on `HashMap<String, Value>` query results, which are well-tested string literals. Adding an extension trait would add unnecessary indirection
- ~~**P3** Replace manual Default impls with `#[derive(Default)]`~~ INVALID: BlockContent has struct variant (#[default] only works on unit variants), Block uses custom values (NO_PARENT_ID, chrono::now()), BlockMetadata already derives Default

### crates/holon-macros/

- [x] **P0** ~~Delete 8 duplicated functions from `lib.rs` that also exist in `operations_trait.rs`~~ DONE
- [x] **P0** ~~Delete 3 truly dead functions from `lib.rs`~~ DONE
- [x] **P1** ~~Add tests for `#[affects]` attribute~~ DONE: 3 tests (single, multiple, empty). Also FIXED BUG: `extract_affected_fields()` only handled `#[operation(affects=[...])]` format, not standalone `#[affects(...)]`. Added parsing for both forms
- [x] **P1** ~~Add tests for `#[triggered_by]` attribute~~ DONE: 3 tests (identity mode, transform mode, multiple triggers on same method)
- [x] **P1** ~~Add tests for `#[enum_from]` attribute~~ DONE: 1 test verifying resolver populates TypeHint::OneOf with correct values
- [x] **P1** ~~Add dispatch function tests~~ DONE: 11 tests (metadata listing, type hints, String/bool/i64/Optional/Value/HashMap dispatch, missing param error, unknown op error, doc comment extraction)
- [x] **P2** ~~Replace brittle string-based attribute parsing with `syn` AST matching~~ DONE: Created `attr_parser.rs` module with proper `syn::Parse` implementations (KeyValue, AttrArgs, Punctuated<LitStr>). Replaced 4 string-based parsers: `extract_entity_attribute` (lib.rs), `extract_param_mappings` (operations_trait.rs), `extract_enum_from` (operations_trait.rs), `extract_affected_fields` (both files). All downstream crates compile and produce identical output
- [x] **P2** ~~Fix type inference to use `syn::Type::Path` matching~~ DONE: Rewrote `infer_type()` to use `syn::Type::Path`, `Type::Reference`, and AST segment matching
- [x] **P2** ~~Enhance error messages in generated dispatch code~~ DONE: All `ok_or_else` errors now include expected type (String, bool, i64, i32, Value)
- [x] **P3** ~~Validate `_id` suffix handling~~ DONE: Changed `strip_suffix("_id")` to `.filter(|s| !s.is_empty())` so bare `_id` falls through to type inference instead of producing empty entity_name

### crates/holon-prql-render/

Cleanest crate in the project. No high-confidence issues found.

- ~~**P3** Investigate unused assignment warnings~~ INVALID: Warnings are from `turso/parser` dependency, not from holon code. `crates/holon/src/core/transform/` doesn't exist. Remaining holon warnings are minor: unused mut in stream_cache.rs (4x), dead code in sql_parser.rs (3 functions for future SQL analysis)
- [x] **P3** ~~Remove `eprintln!` debug output from test functions~~ DONE: Removed 3 assertion-less inspect tests (`inspect_derive_with_render`, `inspect_rq_structure`, `inspect_rq_structure_with_union`, `test_this_star_syntax`), removed eprintln! from 2 tests with assertions (parser.rs, lib.rs), removed 2 `#[cfg(test)] eprintln!` from lineage.rs production code

### crates/holon-orgmode/

- [x] **P0** ~~Fix WriteTracker race condition~~ DONE: Added `hash_bytes()` to file_utils.rs, `mark_write_with_hash(path, hash)` to WriteTracker. OrgFileWriter now computes hash from rendered content BEFORE writing, then marks via `mark_write_with_hash`. Removed dead `mark_our_write` method
- [x] **P1** ~~Add round-trip fidelity tests: parse org file -> render to org -> parse again -> assert equivalence~~ DONE: Added PBT `test_parse_render_parse_fidelity` (parse→render→parse with full field comparison) plus 10 deterministic hand-crafted tests covering simple headlines, TODO/priority/tags, body text, source blocks, named blocks with header args, deep nesting, titles, custom properties, and mixed TODO states
- [x] **P1** ~~Add bidirectional sync integration tests~~ DONE: Added 12 integration tests in `bidirectional_sync.rs` covering: forward sync (pre-existing file, external add/update/delete, source blocks), backward sync (UI create/update/delete → org file), round-trip (external→UI→org and UI→external→backend), stability (no duplicate blocks after UI mutation, rapid updates converge). Also fixed `TestEnvironment::wait_for_block` to include render clause and use 50ms poll interval (was missing render clause causing silent query failures)
- [x] **P1** ~~Add loop prevention tests verifying WriteTracker prevents infinite loops~~ DONE: Added 10 tests in write_tracker.rs: hash matching, modified file rejection, unknown path, mark_write, external processing blocks/unblocks render, cleanup expiry, full loop prevention scenario (write→check→skip), external edit triggers reprocessing, OrgFileWriter waits during OrgAdapter processing
- [x] **P1** ~~Add source block ordering tests (must render BEFORE text children)~~ DONE: Added 3 tests in org_renderer.rs: single source block before child heading, multiple source blocks all before children, interleaved input order still produces correct output
- ~~**P2** Fix `external_processing` race~~ INVALID: OrgFileWriter reads from Loro (not Turso), all Loro ops are synchronously awaited before clearing flag
- [x] **P2** ~~Deduplicate `hash_file()`~~ DONE: Created `file_utils.rs` module with shared `hash_file()`, both callers updated
- [x] **P3** ~~Make debounce window configurable via `OrgModeConfig`~~ DONE: Added `debounce_ms: u64` field to `OrgModeConfig` (default 500), passed through `OrgFileWriter::with_hash_tracking()`, replaced hardcoded `DEBOUNCE_MS` constant
- [ ] **P3** Fully abstract Loro dependency from OrgAdapter - `Option<LoroDocumentStore>` field violates independence claim

### crates/holon-todoist/

- [x] **P2** ~~Add rate limiting to `TodoistClient`~~ DONE: Added `Retry-After` header parsing in `send_with_retry()` for 429 responses. Server-specified backoff is applied before retry. Combined with existing exponential backoff (500ms * 2^attempt) for 429/503/504
- ~~**P2** Add retry logic with exponential backoff for transient network failures~~ INVALID: Already exists in `client.rs` — `send_with_retry()` implements exponential backoff (500ms * 2^attempt) with configurable `max_retries` (default 3). Handles 429, 503, 504 status codes and timeout/connect errors
- [x] **P2** ~~Add inverse operation roundtrip tests~~ DONE: Added 12 tests in `inverse_operation_test.rs`: create→delete inverse, delete→create inverse (with field verification), set_field content roundtrip, set_state completed↔active, set_priority roundtrip, description set/clear. Uses TodoistTaskFake with InMemoryDataSource
- [x] **P2** ~~Validate stream subscriber exists before sync completes~~ DONE: Added `receiver_count()` checks before sending task and project changes. Logs `warn!` with change count when no subscribers exist, explaining that sync token won't be persisted and next sync will re-fetch. Upgraded from silent `info!` to actionable warning
- [x] **P3** Make HTTP timeout configurable via `TodoistConfig` (currently hardcoded 30s at `client.rs:32-37`)
- [ ] **P3** Add sync duration/error metrics for observability

### Frontends (MCP, Blinc, Flutter)

- [x] **P1** ~~Refactor MCP to use `FrontendSession`~~ DONE: Replaced 100 lines of manual DI registration with `FrontendSession::new()`. Also fixed `FrontendSession` to pass `loro_storage_dir` to `OrgModeConfig::with_loro_storage()` when Loro is enabled
- [x] **P1** ~~Add error propagation for CDC stream drops in Flutter FFI~~ DONE: Added explicit `drop(sink)` with warning log when CDC stream ends. Flutter's StreamBuilder receives "done" event when sink is dropped. Previously the info log was misleading — now uses `warn!` level and explicit drop to signal stream termination
- [x] **P1** ~~Add unit tests for CDC forwarding, state management, session initialization across all frontends~~ PARTIAL: Added 12 Blinc AppState unit tests (row CRUD, dirty flag, concurrent reads, clone_handle) and 19 MCP type conversion tests (json↔holon roundtrips, all Value variants, storage entity conversion). FrontendSession integration tests deferred — requires full DI setup with Turso backend
- ~~**P2** Fix MCP HTTP server race~~ INVALID: DI is fully resolved via `.await?` before server starts at line 438
- [x] **P2** ~~Replace `Mutex` with `RwLock` in Blinc AppState~~ DONE: Read-only methods use `read()`, write methods use `write()`
- [x] **P3** Use dynamic port assignment for MCP servers (currently hardcoded 8520 in both Blinc and Flutter - can't run simultaneously) - NOT DONE, we want the same port

### crates/holon-integration-tests/

- [x] **P1** ~~Add CDC stream verification to PBT~~ DONE: CDC pipeline is already verified through `ui_model` built from `drain_cdc_events()` → check_invariants #3 compares ui_model against reference (now with all fields). Enhanced in this session to verify content, content_type, source_language, source_name, parent_id (with document URI normalization)
- [x] **P1** ~~Strengthen convergence assertions - verify Loro and Org file state, not just Turso database~~ DONE: check_invariants already verifies: (1) backend blocks vs reference, (2) Org file blocks vs reference, (3) Loro blocks vs Org file blocks (check #10), (6) structural integrity (no orphans). All three persistence layers are cross-checked
- ~~**P2** Check `PublishErrorTracker`~~ INVALID: Already asserted at line 2294 of general_e2e_pbt.rs
- [ ] **P2** Implement custom PBT shrinking for `Mutation` enum (simplify source blocks to text, remove properties)
- ~~**P2** Remove defensive early return in mutation generation~~ INVALID: Early return at line 1705 is required because `prop::sample::select` panics on empty vec

### crates/holon-filesystem/

- [x] **P2** ~~Fix `OperationProvider::execute_operation`~~ DONE: Implemented dispatch to CRUD, rename, and move_directory operations
- [x] **P2** ~~Fix `DataSource` - `get_all()` and `get_by_id()` always return empty~~ INVALID: QueryableCache<Directory> reads from Turso, not from DirectoryDataSource. The real fix is adding SchemaModule (next item)
- [x] **P2** ~~Add `SchemaModule` for holon-filesystem~~ INVALID: `CoreSchemaModule` already creates directories/files tables at startup. Tables exist.

---

## Strengths Worth Preserving

- **holon-prql-render** is exemplary - clean pipeline, comprehensive tests, excellent architecture alignment
- **Schema Module system** with topological ordering is best-in-class dependency management
- **QueryableCache pattern** correctly implements external-systems-as-first-class principle
- **Operation system** with macro-generated metadata is sophisticated and well-designed
- **Integration test infrastructure** has good PBT support and startup race testing framework
- **holon-frontend** crate provides clean shared abstraction for frontend initialization
- **Source block ordering** in OrgRenderer correctly sorts source blocks before text children
- **CdcCoalescer** in Turso backend intelligently batches DELETE+INSERT into UPDATE to prevent UI flicker
