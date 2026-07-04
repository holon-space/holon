# Architecture Docs Consistency Review (2026-07-06)

Senior-architect audit of `docs/Architecture/*` (excluding Model.md and CrateMap.md,
covered by other reviewers) plus `docs/adr/BLOCK_LORODOC_ARCHITECTURE.md`, checked
against the actual code after the "Petri-net based engine refactor: complete cleanup —
remove old reactive types" merge (30127e8e12).

**Primary question asked:** do these docs still describe a "reactive" world the Petri
refactor deleted? **Answer: no.** The refactor removed `AppState`, `spawn_ui_listener`,
`CdcState`, `BlockWatchRegistry`, `block_watch()`, `widget_states`, and the
`ReactiveViewKind` enum from `holon-frontend`. None of the audited docs reference those
deleted symbols as live. `ReactiveViewModel` itself still exists
(`crates/holon-frontend/src/reactive_view_model.rs`, rewritten to a persistent-node
architecture) and the docs describe the new shape correctly. The docs were last touched
2026-07-04 and are, overall, unusually well-maintained.

## Severity summary

| Severity | Count | Docs |
|----------|-------|------|
| Critical | 0 | — |
| Major | 3+4 | Sync.md (operations→operation table, 2 runnable examples + prose); c4/baseline stale (3 crates missing + Loro extraction invisible) |
| Minor | ~18 | spread across Engine, Operations, Sync, Replication, Storage, UI, Integrations, Principles, Archlint, ADR |
| Stale-term / cosmetic | several | enum ordering, flutter dir, example spelling, line counts |

Two highest-impact issues:
1. **`operations` vs `operation` table name in Sync.md** — an agent copying the PRQL
   examples would query a non-existent table.
2. **c4 diagrams + `baseline/crates/architecture.json` are one refactor behind** — they
   miss `holon-loro`, `holon-petri`, `holon-profiles` and still show the pre-extraction
   `holon/src/sync/` Loro file set. Mechanical fix: re-run `just arch-docs`.

Everything else is wording refinement or completeness nits.

---

## Engine.md — ~95% accurate, not describing deleted code

| # | Section (line) | Claim | Reality (file:line) | Severity | Suggested edit |
|---|---|---|---|---|---|
| 1 | Supported Frontends (L60) | "Flutter \| Deprecated \| Directory removed" | `frontends/flutter/` still exists (stale build artifacts, tracked source gone) | Minor / stale | "Source removed; only stale build artifacts remain" or delete the dir |
| 2 | Key Components (L28) | "`ObjectiveResult` \| `objective.rs` \| Evaluates objective function" | `ObjectiveResult` is the struct (`objective.rs:4`); evaluator is free fn `evaluate(...)` (`objective.rs:9`) | Minor | Name the component `evaluate()` returning `ObjectiveResult` |

Verified CORRECT (high-stakes): Core Traits block (L14-17) exact match to
`holon-engine/src/lib.rs:23-57` (TokenState/TransitionDef/NetDef/Marking, every method).
`holon-engine` has no dep on the `holon` crate — "standalone, YAML-driven" holds.
`RhaiEvaluator` in `guard.rs`, `Engine`/`RankedTransition` in `engine.rs` with
`enabled()`/`fire()`/`rank()`. `holon-petri` deps and module doc match.

## Operations.md — ~97% accurate, well-hedged

| # | Section (line) | Claim | Reality | Severity | Suggested edit |
|---|---|---|---|---|---|
| 1 | Examples (L33/54/62 vs L362/529) | entity spelled `todoist-task` / `todoist-tasks` / `todoist_tasks` interchangeably | Real derive uses underscores; example naming inconsistent (cosmetic) | Minor | Pick one spelling |
| 2 | Op table schema vs struct (L150 vs L158-174) | SQL has `_change_origin TEXT` | `OperationLogEntry` struct listing (L161-173) omits `_change_origin` | Minor | Note how `_change_origin` maps, or add to struct listing |

Verified CORRECT (high-stakes): `OperationResult` and `OperationDispatcher` field lists
exact (`traits.rs:259-265`, `operation_dispatcher.rs:31-35`). All four routing providers
exist and named as claimed (`LoroBlockOperations`, `SqlOperationProvider`,
`McpOperationProvider`, `OrgModeSyncProvider`) + `RegistryOperationProxy`. `UndoStack`
default 100, `DispatchingOperationEngine` holds `Arc<RwLock<UndoStack>>`. **Write-only-log
claim holds**: `mark_undone`/`mark_redone` have callers only in tests. Cells section
(`BlockCellRegistry`, `live_field_any`, `LwwScalarBacking<T>`, `LoroMetaCellBacking<T>`)
all present; status caveats match reality.

## RenderPipeline.md — fully current, unusually well-maintained

No Critical/Major/Minor findings. Every named type, trait, file path, function verified.

Verified CORRECT (high-stakes): `ReactiveViewModel` struct (L174-191) matches
`reactive_view_model.rs:304-367` field-for-field (`expr: Mutable<RenderExpr>`, `data:
ReadOnlyMutable<Arc<DataRow>>`, children/collection/slot/…/props/render_ctx/interpret_fn/
subscriptions). `watch_ui`/`merge_triggers`/`switch_map` all present (`ui_watcher.rs:48,100,180`).
Profile resolution, core types (`EntityProfile`, `StoredVariant`/`RowVariant`,
`RenderProfile`/`RowProfile`), `DataRow`/`DataRowAccumulator` in `widget_spec.rs`,
`ReactiveRowSet::apply_change` — all verified. The only deleted-type reference
(`ReactiveViewKind`) is the code's own lineage doc-comment, mirrored correctly as history.

## UI.md — accurate and current

| # | Section (line) | Claim | Reality (file:line) | Severity | Suggested edit |
|---|---|---|---|---|---|
| 1 | State table (L12) | viewport a `Mutable` on `UiState` | viewport is a separate frontend-owned struct (`reactive.rs:934`); `UiState` only holds `viewport_generation:959` | Minor | Note viewport lives in its own struct |
| 2 | Chord ops (L48) | "archlint bans `BlockContentResolver`'s return" | `BlockContentResolver` deleted (0 hits); archlint bans the family `with_content_resolver`/`EditableTextProvider`/`live_content` (`archlint/smells/words.toml`) | Minor | Reword to the content-resolver family, not the literal symbol |

Verified CORRECT: `editable_text(uri,field) -> Cell<String>`, full Cell API
(`current()`, `apply_text_op(TextOp)`, `anchor_cursor`/`resolve_cursor`, `remote_deltas()`),
`read_content_via_cells` → `cell.current()`, write via `BlockCellRegistry::write_field`,
`ReactiveViewKind` framed as replaced-history, GPUI `ui_generation` bumps but focus does not.

## Sync.md — high accuracy; ONE Major (table name)

| # | Section (line) | Claim | Reality (evidence) | Severity | Suggested edit |
|---|---|---|---|---|---|
| S1 | Op Log schema (L596-607) | `CREATE TABLE operations (...)` | Real table is **`operation`** (singular): `#[entity(name = "operation")]` `operation_log.rs:65`; all SQL uses `operation`. Columns match. | **Major** | Rename to `operation` (a stale `// Table name: operations` comment exists in code too) |
| S2 | UI PRQL example (L681-687) | `from operations` | Table is `operation`; PRQL would fail | **Major** | `from operation` |
| S3 | UI prose (L689) | "when the `operations` table changes" | Same mismatch | Minor | `operation` |
| S4 | Components (L148) | `SqlOperationProvider` = "SqlOnly fallback for blocks" | The block `OperationProvider` is `SqlBlockOperations` (`sql_block_operations.rs`) wrapping `SqlOperationProvider` | Minor | Add `SqlBlockOperations` row |
| S5 | Loro data model (L135/357/371) | `properties` = "JSON-serialized custom properties" | Properties are a nested per-property `LoroMap` (per-key LWW) per L357; field-table wording at odds | Minor | Align field-table wording with L357 |

Verified CORRECT (high-stakes): `TREE_NAME="blocks"` single `LoroTree`; `LoroBackend`
fields incl. `id_cache: Arc<Mutex<HashMap<String, TreeID>>>` (stable-block-ID refactor
reflected). `block_raw` is the physical table, `block` a matview. `BlockConsolidator`
single writer via `execute_batch_with_origin(..., EventOrigin::Loro)`. **No EventBus**:
`TursoEventBus`/`EventBus`/`CollaborativeDoc` all gone (0 hits). P2P split
(`IrohSyncAdapter`, `LoroShareBackend`, `multi_peer`). `run_block_mirror` deleted;
`BlockFeed`/`LiveData<Block>`/`LinkEventSubscriber::start_from_live_data`. `FileFormatAdapter`
+ `OrgFormatAdapter`/`MarkdownFormatAdapter`, `BlockContent` enum all match.

## Replication.md — explicitly "target arch (2026-05)", caveats honest

| # | Section (line) | Claim | Reality (evidence) | Severity | Suggested edit |
|---|---|---|---|---|---|
| R1 | §5 (L272-273) | "`loro_seams.rs` placeholder … has been deleted" | File `holon-app/src/loro_seams.rs` still exists (`LoroBlockOrdering`, `BlockOrdering`-only, no minting). Only the minting placeholder is gone. | Minor / stale | Clarify: minting placeholder removed, seam file remains |
| R2 | §5 status (L291-296) | fi fallback "in `loro_sync_controller.rs`" | `unwrap_or_else(default_sort_key)` lives in `loro_backend.rs` read path; the R-1 `debug_assert` + null-sort-key PBT invariant ARE in `loro_sync_controller.rs:746-753` | Minor | Attribute fi fallback to `loro_backend.rs`, keep the R-1 guard citation |
| R3 | §7 (L355-356) | per-parent `SignalVec` "only in command-palette popup" | Also used in `link_provider.rs` + defined as `rows_signal_vec` trait method in `interp_value.rs` | Minor | Broaden scope |

Verified CORRECT (high-stakes): `BaseStore`/`SyncBaseStore` keyed by `BaseKey{peer,file}`
storing `HashMap<String, SnapshotBlock>` matches §3. `ChangeOp::Relocate{id,parent,after_sibling}`
with no order-key field + disclosed `sort_key`-param fallback (`change_set.rs:111,276`).
`LoroTextMergeProvider`/`TransientTextMergeProvider`. `OrderKeyMinting` implemented solely
by `SqlBlockOperations`; `LoroBlockOrdering` impls only `BlockOrdering` — the type-enforced
"minting unrepresentable on Loro path" claim is real.

## Storage.md — factually accurate, freshly maintained

| # | Section (line) | Claim | Reality (evidence) | Severity | Suggested edit |
|---|---|---|---|---|---|
| 1 | Cell Registry (L62-72) | `CellBacking<T>: Send + Sync` | Actual: `Cell<T: 'static+Send+Sync+Clone>`, `CellBacking<T: ...Clone>: Send+Sync+'static` (`cell.rs:27,73`) | Minor | Add `: Clone` bound or note bounds elided |
| 2 | Change Origin (L305-310) | enum `Remote` then `Local` | Code: `Local` then `Remote` (`streaming.rs:39-53`) | Cosmetic | Reorder |
| 3 | Platform Support (L215) | Unix-like incl. Android | `turso.rs:1096` doc lists macOS/Linux/BSD/iOS (Android via UnixIO, functionally correct) | Minor | Harmonize |

Verified CORRECT: `TursoBackend`/`DbHandle` with `cdc_broadcast`/`cdc_seq`;
`QueryableCache<T>` + `QueryableCache<Block>` wiring; `apply_batch` live callers;
`ingest_stream*` defined with zero live call sites (correct); `coalesce_row_changes`/
`row_changes`; `RowChange`/`ChangeData`/`Change::FieldsChanged`/`ChangeOrigin::{Local,Remote}`;
`get_version`/`set_version` over `_version`; Windows unsupported. CDC-fires-only-via-matviews
consistent.

## Schema.md — zero factual errors found

No findings. Every table, column, matview definition, PK/FK, and ownership-module claim
checks out against `crates/holon-turso/sql/schema/*.sql` and `schema_modules.rs`.

Verified CORRECT (high-stakes): `block_requirement_edges` joins the `block` matview (not
`block_raw`) — chained matview confirmed. `block_raw` 16-column list matches `blocks.sql`.
`block` matview dual-JOIN + `json_group_array FILTER` + `COALESCE`. `block_with_path`
recursive CTE reads `FROM block`, root detection `parent_id LIKE 'sentinel:%'`, no legacy
`doc:%` branch. All 11 registry module structs exist verbatim. Junction tables, navigation
matviews, graph_eav schema, `TryFrom<StorageEntity> for Block`, `BlockWire`,
`create_entity_type` MCP tool — all match.

(Non-doc note: `block_matview.sql` internal comment says "All 17 columns of block_raw" but
block_raw has 16 — stale comment inside the SQL, not in the doc.)

## Integrations.md — actively accurate, self-aware about aspirational parts

| # | Section (line) | Claim | Reality | Severity | Suggested edit |
|---|---|---|---|---|---|
| 1 | L140 | `normalize_var_name` (crate ambiguous) | Lives in `holon-app/src/mcp_integrations.rs:20`, not `integration_config.rs` (holon-mcp-client) | Minor | Disambiguate crate |

Verified CORRECT (high-stakes): MCP-Apps sections carry explicit "Status: target
architecture" banners and correctly state `AppBridge`/`McpAppView` do not exist (0 hits) —
honest-degradation pattern. `_fdw`/`write_through` FDW mechanics (`mcp_integration.rs:555,568`).
Full YAML sidecar schema matches `mcp_sidecar.rs`/`integration_config.rs`/`mcp_sync_strategy.rs`
and `docs/integrations/todoist.yaml`. `holon-todoist` crate deleted. All 9 component
types+files resolve. Frontend signatures block matched verbatim (`render_entity`,
`watch_ui`→`WatchHandle`, `execute_operation`, `ReactiveEngine::watch`).

## BlockEventStorm.md — historical doc for a CLOSED track, self-labeled

No Critical/Major findings. The doc explicitly self-labels as historical ("Track closed
2026-07-02… milestones M0-M7 done", L6-9/L155-160) — no reader would mistake it for live
work. Never references deleted `space.holon` (that belonged to a different track).

Verified CORRECT: H2 `Create.parent_id: EntityUri`, `Relocate.parent: Option<EntityUri>`,
`after_sibling`, `decode_create`/`decode_update` with fail-loud non-string parent panic
(`change_set.rs:97,113,234,244`). H3 `PROPERTIES_MAP` nested LoroMap. H7 `PAGE_TAG="Page"`
+ ADR 0014 doc-scheme retirement. H12 `blocks_differ` over `EdgeField::ALL`. H1 `BlockWire`
serde-free block.

## Principles.md — substantively accurate

| # | Section (line) | Claim | Reality (evidence) | Severity | Suggested edit |
|---|---|---|---|---|---|
| 1 | DI example (L606) | `injector.add_mcp_server(8520)?;` | No `add_mcp_server`/`add_mcp*` method exists (0 hits); MCP wiring lives in `holon-app/src/mcp_integrations.rs` | Minor | Replace with the real MCP registration call or drop the line |
| 2 | Cells (L390-392) | `MutableText`/`BlockCellRegistry` (implied holon-frontend/core) | Actually live in the extracted `holon-loro` crate | Minor | Optionally name the crate |

Verified CORRECT: `FrontendSession`+`ReactiveViewModel` (`reactive_view_model.rs:304`);
`SyncBaseStore` at `holon-filesystem/src/sync_base_store.rs` + `sync_conflict.rs`/`sync_ports.rs`;
`TransientTextMergeProvider` in `holon-loro`; `LoroSyncController`+`BlockConsolidator`;
`HOLON_CRDT_ENABLED` toggle (`config.rs:127`, `crdt_enabled():436`); `compile_query` exact
signature. AI-services/privacy sections explicitly marked aspirational. No deleted-reactive
references.

## Archlint.md — most accurate doc of the set

| # | Section (line) | Claim | Reality | Severity | Suggested edit |
|---|---|---|---|---|---|
| 1 | File layout (L70-100) | `archlint/` tree | Undocumented `aggregates/` + `discoveries/` dirs omitted (`aggregates/` backs `no-scattered-match-as-str`, L244) | Minor | Add both dirs to the tree |
| 2 | Related (L269) | "the 51-line cargo wrapper" | `architecture_rules.rs` is 52 lines | Stale-term | 51 → 52 |

Verified CORRECT: `archlint.py` exactly 870 lines; all rules/ (6), smells/ (7), dylint/ (5)
files present; full ~40-id rule table matches `rg '^id'` across smells/rules; `archlint_all_passes`
test execs `archlint --all` and panics on nonzero.

## c4/ diagrams + baseline/ JSON — one major refactor behind (Major)

| # | Artifact | Claim | Reality (evidence) | Severity | Suggested edit |
|---|---|---|---|---|---|
| 1 | `baseline/crates/architecture.json` | 20 crate dirs | Workspace has 23. Missing **holon-loro**, **holon-petri**, **holon-profiles** | **Major** | Regenerate baseline |
| 2 | `c4/crates/diagrams/*.puml` | crate inventory | Same 3 crates missing | **Major** | Regenerate diagrams |
| 3 | baseline `holon/src/sync/` (L462-570) | ~30 Loro files (consolidator, loro_sync_controller, text_merge_provider, shared_tree…) | Moved to new `holon-loro` crate; `holon/src/sync/` now holds 5 files | **Major** | Regenerate — the whole Loro extraction is invisible |
| 4 | baseline holon→holon-engine edge (L31) | "Petri-net engine" | `holon-engine` = standalone WSJF/YAML CLI; the integrated Petri engine is `holon-petri` (absent). `holon` deps on both | **Major** | Add holon-petri node/edge; relabel holon-engine edge "standalone Petri CLI" |
| 5 | `baseline/frontends/architecture.json` | 9 frontend dirs | CORRECT — matches `frontends/` exactly | — | none |

The frontends baseline is current; the crates baseline + all crate-level c4 diagrams predate
the holon-loro extraction and holon-petri/holon-profiles additions. Fix is mechanical: re-run
the generator (`just arch-docs`). What the baseline does contain (spot-checked: holon-api enums,
holon-core traits, holon-frontend ReactiveViewModel, holon-engine desc) is accurate.

## BLOCK_LORODOC_ARCHITECTURE.md (ADR) — sound as a superseded/rejected doc

| # | Section (line) | Claim | Reality (evidence) | Severity | Suggested edit |
|---|---|---|---|---|---|
| 1 | Collaboration model (L112-120) | "mount nodes" (part of rejected Option D) | Mount-node machinery actually SHIPPED: `holon-loro/src/shared_tree.rs` (`is_mount_node`, `read_mount_info`, used by `loro_backend.rs:14`) | Minor (observational) | Footnote that mount nodes survived into the shipped all-in-tree design |

Verified CORRECT: the prominent self-disclaimer (L7-14, "rejected proposal, does NOT describe
the shipped system, read ADR 0003") matches reality — code uses a single LoroTree in one LoroDoc,
block content in each node's meta LoroMap, stable UUID `id` + `id_cache`, per-property nested
LoroMap LWW. No `content_doc_id`/per-block LoroDoc/`block_snapshots` anywhere.

---

## Cross-document consistency (auditor's own pass)

- **`Architecture.md` index (root, out-of-assigned-scope but high-visibility)** still contains
  an old "Reactive Data Flow" diagram (`UI <- futures-signals ← CDC Stream ← QueryableCache
  ← Sync Provider`) and a "Streaming-first render state" note referencing `ReactiveRenderedRows`
  + `watch_ui`. `ReactiveRenderedRows` was NOT found in `crates/` (0 hits) — the type appears
  renamed (`ReactiveRowSet`/`ReactiveViewModel`). This is the first thing a reader sees; flag
  for a follow-up pass.
- **Crate/frontend inventory drift**: `Architecture.md`'s crate tree omits `holon-petri` and
  `holon-profiles` from the tree block but DOES list holon-petri in the Key-files table (L270).
  `frontends/flutter/` exists on disk but is absent from the doc's frontend list; conversely
  Engine.md says it was "removed" — the two docs disagree about flutter.
- **Table-name consistency**: Sync.md says `operations`; Schema.md and Storage.md correctly use
  `operation` (singular). The singular form is correct — Sync.md is the outlier.
- **Self-labeling discipline is strong**: Replication.md, Integrations.md (MCP-Apps), and
  BlockEventStorm.md all carry dated "target architecture" / "track closed" banners that match
  reality — the project's "fail loud, never fake" ethos is visible in the docs.

## Method note

Six parallel verifier agents (one per doc cluster), each grounding high-stakes claims against
code via `ast-outline` + `rg` + targeted reads. The reviewer independently ran the cross-doc
consistency pass and the root-index / inventory drift checks above.
