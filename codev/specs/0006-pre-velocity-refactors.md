# Pre-Velocity Architectural Refactors

**Status**: Plan
**Date**: 2026-04-26
**Goal**: Land the smallest set of architectural refactors needed before shifting into parallel feature work, so feature code doesn't get rewritten and PBTs don't break in the next big refactor cycle.

## Why this exists

The E2E PBTs in `crates/holon-integration-tests/tests/general_e2e_pbt.rs` have stabilized to the point where they catch most regressions. Refactor frequency is dropping. The user wants to know whether shifting into feature velocity is safe **now**, or whether vision-driven features still ahead will force more architectural rewrites that invalidate tests.

A vision-vs-architecture audit (Vision/*.md, Architecture/*.md, FIRST_RELEASE_FEATURES.md, Strategy/MVPs.md) produced this verdict: **the architecture supports the vision, but three seams aren't quite finished**. Each is small. Each is on the critical path of multiple first-release features. Skipping them would mean rewriting feature code and PBT assertions in the next refactor cycle.

This plan lands those seams in dependency order, then opens the gate to parallel feature work.

## The phases

### Phase 0 — Stabilize ReactiveViewModel signal propagation [DONE 2026-04-26]

The architectural fix landed:

- `ReactiveRowSet` single-writer (only `apply_change` writes through `Mutable`)
- `ReadOnlyMutable` everywhere downstream — type-level enforcement of "one writer = `ReactiveRowSet`"
- Leaf builders subscribe to the per-row data signal via `DropTask`:
  - `state_toggle.rs`, `editable_text.rs` — full subscription pattern
  - `expand_toggle.rs`, `selectable.rs` — share the data handle (no subscription needed — PK-column derived)
  - `pref_field.rs` out of scope (uses container row set, not per-row binding)

Verified by `reactive_view_model::tests::shared_data_cell_updates_propagate_to_state_toggle_child` (green) and PBT `inv10h_live` (0 divergences on baseline runs). `LAYOUT_MUTATIONS_ENABLED` flag in `crates/holon-integration-tests/src/pbt/state_machine.rs` re-enabled (was disabled while investigating the bug).

**Two PBT-blocking bugs surfaced as separate tasks** — both documented in MEMORY as "open issues unrelated to the toggle bug", confirmed today by running the full suite:

- Task #7: Chord dispatch for block tree ops (`indent`/`outdent`/`move_up`/`move_down`/`split_block`) doesn't fire — the keychord lookup walks the focus path but no node carries the matching keychord-triggered op.
- Task #8: `inv16` panics — editable_text descendants of a `focus_roots(Main)` leaf don't get wrapped in `draggable()` because they aren't in the panel's query result rows.

Both block Phase 5.

### Phase 1 — Extract `FileFormatAdapter` from `OrgSyncController`

Generalize `crates/holon-orgmode/src/org_sync_controller.rs` (971 lines) into a `FileFormatAdapter` trait + adapter use in the controller. Org becomes the first impl, behavior unchanged.

#### Step 1.1 — Trait + adapter wrapper [DONE 2026-04-26]

- `crates/holon-core/src/file_format.rs` defines `FileFormatAdapter` and `FileFormatParseResult` (format-neutral).
- `crates/holon-orgmode/src/file_format.rs` provides `OrgFormatAdapter`, a stateless wrapper that delegates to `parse_org_file` and `OrgRenderer`.
- `holon-orgmode/Cargo.toml` adds `holon-core` as a direct dep.
- Compile clean.

Trait shape (final):

```rust
pub struct FileFormatParseResult {
    pub document: Block,
    pub blocks: Vec<Block>,
    pub blocks_needing_ids: Vec<String>,
}

pub trait FileFormatAdapter: Send + Sync {
    fn extensions(&self) -> &'static [&'static str];
    fn parse(&self, path: &Path, content: &str, parent_dir_id: &EntityUri, root: &Path) -> Result<FileFormatParseResult>;
    fn render_document(&self, doc: &Block, blocks: &[Block], file_path: &Path, file_id: &EntityUri) -> String;
    fn render_blocks(&self, blocks: &[Block], file_path: &Path, file_id: &EntityUri) -> String;
}
```

Differences from the original sketch in this spec: no `watch()` (file watching stays in the controller — it's format-agnostic), no `echo_suppress_origin()` (also generic plumbing), parse takes `&str` not `&[u8]` (matches the existing org parser), render returns `String` not `Result<Vec<u8>>` (matches `OrgRenderer`).

#### Step 1.2 — Refactor `OrgSyncController` to call through the adapter [DONE 2026-04-26]

`crates/holon-orgmode/src/org_sync_controller.rs` no longer calls `parse_org_file` or `OrgRenderer::*` directly. The controller now holds `format: Arc<dyn FileFormatAdapter>` and delegates parse + render through it.

Changes:
- `OrgSyncController` gained a `format` field.
- `OrgSyncController::new(...)` (signature unchanged) now wraps `Self::with_format(..., Arc::new(OrgFormatAdapter::new()))`. `di.rs` and all existing call sites work as-is.
- `OrgSyncController::with_format(..., Arc<dyn FileFormatAdapter>)` is the new explicit-format constructor — markdown / notion / logseq wirings will use this.
- 6 call sites swapped: 2 in `initialize`, 2 in `on_file_changed`, 2 in `render_file_by_doc_id`.

Verification:
- `cargo check -p holon-orgmode` clean.
- 3 new unit tests in `crates/holon-orgmode/src/file_format.rs` prove parse + render delegation is byte-identical to the underlying functions (`parse_returns_same_document_and_blocks_as_underlying_parser`, `render_blocks_matches_underlying_renderer`, `extensions_returns_org`).
- The two `bidirectional_sync` tests that exercise the render path via the adapter (`backward_sync_ui_update_writes_to_org_file`, `backward_sync_ui_delete_removes_from_org_file`) pass.
- The 10 other `bidirectional_sync` tests failing are pre-existing (signaled by the `unused_variable: startup_errors` warning at `crates/holon-integration-tests/src/test_environment.rs:330` — the test environment silently drops startup errors). Logged as a separate concern; not introduced by this refactor.

The image-handling paths (`materialize_images`, `ingest_images`) and the `post_org_write_hook` are arguably format-specific too, but they don't block Phase 1. They can stay where they are or move to optional adapter methods later — **defer that decision until a markdown adapter exists** to inform the right shape.

**Files touched**: `crates/holon-core/src/file_format.rs` (new), `crates/holon-core/src/lib.rs`, `crates/holon-orgmode/Cargo.toml`, `crates/holon-orgmode/src/file_format.rs` (new), `crates/holon-orgmode/src/lib.rs`, `crates/holon-orgmode/src/org_sync_controller.rs`.

**Follow-up**: update `docs/Architecture/Sync.md` to document the seam.

#### Step 1.3 — Second adapter: `MarkdownFormatAdapter` (validates the seam, ships F1)

`crates/holon-markdown` (new crate) provides `MarkdownFormatAdapter`, the second `FileFormatAdapter` impl. Targets Obsidian-style vaults: CommonMark + GFM task lists + YAML frontmatter + `[[wikilinks]]` + `^block-id` markers + fenced code blocks (mapped to `ContentType::Source` children, mirroring org's `#+BEGIN_SRC` mapping).

- `crates/holon-markdown/src/parser.rs` — `parse_markdown_file` returns `ParseResult { document, blocks, blocks_needing_ids }` matching the org parser's shape exactly.
- `crates/holon-markdown/src/renderer.rs` — `MarkdownRenderer::render_document` / `render_blocks` mirroring `OrgRenderer`. Source children render before text children of the same parent (same ordering rule org uses) so the next parse re-attaches them to the correct heading.
- `crates/holon-markdown/src/frontmatter.rs` — YAML frontmatter projected onto typed `title` / `tags` fields with arbitrary keys preserved verbatim under `frontmatter_extra` for round-trip fidelity.
- `crates/holon-markdown/src/wikilink.rs` — `[[wikilink]]` extraction. Targets are stored on the block under a `wikilinks` JSON-array property; raw `[[…]]` text stays in `block.content` so round-trip is byte-stable. **Resolution to `EntityUri::file(...)` is intentionally a higher layer's job** — the adapter is single-file scope and doesn't have the vault-wide filename index needed to map a name to a path.
- `crates/holon-markdown/src/file_format.rs` — `MarkdownFormatAdapter` impl, plus 4 unit tests including a full parse → render → reparse round-trip with stable IDs.
- `crates/holon-markdown/tests/seam_validation.rs` — drives the adapter through `Arc<dyn FileFormatAdapter>` to prove the trait surface is the seam.

Verification: 37 unit tests + 2 integration tests, all green. `cargo check -p holon-orgmode -p holon-markdown` clean (only pre-existing warnings).

**Deferred decisions, now resolved by the second impl**:

- **Image handling**: stays on the controller. Both formats carry image children as `ContentType::Image` blocks with a relative path on `block.content`; the disk-side materialize/ingest is identical and format-agnostic. The format-specific bit is only the *syntax* (org's `[[file:path.png]]` vs markdown's `![[path.png]]`), which already lives in each adapter's parser/renderer.
- **`post_org_write_hook`**: stays on the controller; rename to `post_write_hook` is a follow-up cleanup (the hook applies equally to a vault, e.g. Obsidian plugin reload). Not needed for landing this adapter.

**Files touched (step 1.3)**: `crates/holon-markdown/` (new crate, 6 source files), `Cargo.toml` (workspace members + internal deps), `docs/Architecture/Sync.md` (documents the second impl + the deferred-decision verdict).

**Why this lands now (F1)**: per `docs/FIRST_RELEASE_FEATURES.md`, F1 (Obsidian Vault as Data Source) is the single highest-impact tier-1 feature. The adapter is the wedge — once it's in place, vault watching, change detection, and bidirectional write-back all flow through the existing `OrgSyncController::with_format(...)` path with zero controller changes.

**Why**: F1 (Obsidian), LogSeq, Notion-import, and every Goals.md priority integration (Gmail, Calendar, JIRA, Linear, GitHub, Notion) all need this. Building any of them on a copy-paste of org sync forces a rewrite once the second one arrives.

### Phase 2 — Render DSL widget registry seam [DONE 2026-04-26]

**Decision: Option (a) — keep the existing `RenderExpr::FunctionCall { name, args }` shape and finish the seam by making widget-name registration self-bootstrapping.**

The audit showed the enum was already correct: every widget is parsed as a `FunctionCall` with a free-form `name` string; the frontend's auto-generated `builder_registry!` macro resolves the name at *interpret* time. Adding a new widget like `kanban` already required no enum or parser change — only a new builder file in `crates/holon-frontend/src/shadow_builders/`. The bug was that the parse-time name registry was a `OnceLock` that **panicked** when callers (backend tests, headless engines, action_watcher) couldn't reach the frontend's `register_render_dsl_widget_names()`. Multiple tests were failing with `register_widget_names() must be called before any render DSL parsing`.

**Implementation** (`crates/holon/src/render_dsl.rs`):

- Added `extract_function_names(source)` — scans the Rhai source for identifiers in function-call position (`name(`), skipping string literals and `//` comments, filtering out Rhai reserved words.
- `parse_render_dsl(source)` and `parse_render_dsl_with_names(source, names)` now union pre-registered names with names extracted from the source itself, then build a Rhai engine with all of them. New widgets parse correctly even when `register_widget_names` was never called.
- `registered_widget_names()` no longer panics — returns `&[]` when unset. The frontend still calls `register_widget_names()` at startup as a *hint* (faster, avoids re-scanning), but it's no longer load-bearing.
- Tests added: `new_widget_parses_without_explicit_registration` (the exit criteria — `kanban(#{...})` parses with zero registration), `nested_new_widgets_parse_without_registration`, `extract_function_names_skips_string_literals_and_comments`, `extract_function_names_skips_rhai_keywords`.

**Verification**:

- `cargo test -p holon --lib render_dsl` — 13/13 green.
- `cargo test -p holon --features test-helpers --test json_aggregation_e2e_test` — 3/9 originally panicking tests now pass; the remaining 6 failures are unrelated (`NOT NULL constraint failed: directory.parent_id`, JSON aggregation derived-column behavior). **Zero remaining occurrences of the registry panic** (`grep -c "register_widget_names() must be called"` → 0).
- `cargo check -p holon-frontend` clean.

**Exit criteria met**:

- ✅ Kanban/calendar/parametric-style can be added by registering a new builder; no enum or parser change required.
- ✅ No `RenderExpr` enum variants added; no Rhai parser changes; no frontend builder-map shape changes.
- ✅ PBT `DisplayNode` assertions in `display_assertions.rs` unchanged — they already operate on `FunctionCall { name, args }` directly.

**Files touched**: `crates/holon/src/render_dsl.rs`.

### Phase 3 — Decide entity-type-system landing [DONE 2026-04-26]

**Decision: SHIP. Marked Implemented.**

Audit found the unified surface was already in code: `TypeRegistry` (`crates/holon/src/type_registry.rs`), `TypeDefinition` (`crates/holon-api/src/entity.rs`), `FieldLifetime`, `DynamicSchemaModule` for runtime DDL, YAML loaders for `assets/default/types/*.yaml`, `create_entity_type` MCP tool for runtime registration, and pre-compilation of computed expressions at registration time. `Schema`, `HasSchema`, `EntitySchemaProvider`, `EntitySchema`, `EntityFieldSchema`, and the standalone `ComputedField` struct are all gone. The 32-day-old `entity_type_system_design.md` memory entry that listed all 5 phases as "COMPLETE" was accurate; only the architecture doc was stale.

**Carve-out, kept intentionally separate**: the Petri WSJF scoring engine (`crates/holon/src/petri.rs`) keeps its own `PrototypeValue::{Literal, Computed}` and `resolve_prototype` numeric merge. The two mechanisms operate on different domains — entity-row enrichment over `StorageEntity` vs prototype/instance/context f64-only merge for ranking — and unification would force the petri ranking path through a typed-entity surface that doesn't fit. Documented in `docs/Architecture/Schema.md` §"Computed Fields and Prototype Blocks" as an explicit non-goal.

**Files touched**:

- `docs/Architecture/Schema.md`: removed "(Partially Implemented)" from heading; added "Implementation Status" section; rewrote "Computed Fields and Prototype Blocks" section as a side-by-side comparison of `FieldLifetime::Computed` vs `PrototypeValue::Computed` with the carve-out documented.
- Anchor `#entity-type-system-partially-implemented` → `#entity-type-system` updated at the two intra-doc references (Schema Module System and Schema System sections).

**Verification**:

- `cargo check -p holon -p holon-api` clean (one pre-existing unused-import warning, unrelated).
- `docs/Architecture.md` reviewed — no stale "partially implemented" language.

**Exit criteria met**: Schema.md says "Implemented", PBT shape unchanged (no `Schema`/`HasSchema` references in tests; type-registry-driven flows already live).

### Phase 4 — Entity Identity skeleton

Land **only** the schema and operation surface for canonical entity identity. No AI, no UI, no classifier — just the seam, so every future integration plugs in instead of hard-coding its own identity column.

**Schema**:

- `canonical_entity (id, kind, primary_label, created_at)`
- `entity_alias (canonical_id, system, foreign_id, confidence)` — maps Todoist/JIRA/Gmail/etc IDs to canonical
- `proposal_queue (id, kind, evidence_json, status, created_at)` — empty by default

**Operations** (in `OperationDispatcher`):

- `merge_entities(canonical_a, canonical_b)` with full undo
- `propose_merge(evidence)` — appends to queue, no side effects
- `accept_proposal(id)` / `reject_proposal(id)`

**Exit criteria**: tables exist, three operations land with PBT coverage on merge + undo. Other PBTs unchanged (table starts empty). `Architecture/Schema.md` gets a section on identity.

**Why**: Vision/LongTerm "Zeroth Principle" + AI/Integrator role both demand canonical identity across systems. Each new integration crate written without it grows ad-hoc identity columns that need ripping out later.

#### Step 4.1 — Schema seam [DONE 2026-04-26]

Tables, SchemaModule, DI wiring, and docs landed. Operations come in step 4.2.

- `crates/holon/sql/schema/identity.sql` — three tables + indexes (`canonical_entity`, `entity_alias`, `proposal_queue`). `entity_alias` PK is `(system, foreign_id)`; `entity_alias.canonical_id` REFERENCES `canonical_entity(id)`.
- `crates/holon/src/storage/schema_modules.rs` — `IdentitySchemaModule` (no deps; provides the three table resources). Module-API unit test added.
- `crates/holon/src/storage/mod.rs` — re-exports `IdentitySchemaModule`.
- `crates/holon/src/di/schema_providers.rs` — `IdentityTables` `DbResource` marker + `DbReady<IdentityTables>` provider.
- `crates/holon/src/di/registration.rs` — both `BackendEngine` providers (Unix and WASM paths) gain `.with_dependency::<DbReady<IdentityTables>>()` so every engine boot creates the tables.
- `crates/holon/tests/identity_schema_smoke.rs` (new) — three tokio integration tests: schema creates the three tables, basic insert round-trip, `(system, foreign_id)` PK rejects duplicates. All green.
- `docs/Architecture/Schema.md` — new "Entity Identity" section with the DDL, SchemaModule wiring, and a note that the operations are scheduled but not yet landed.

`cargo check -p holon` clean. `cargo test -p holon --test identity_schema_smoke` 3 passed.

#### Step 4.2 — Operations + PBT [DONE 2026-04-26]

Operations land as a new `IdentityProvider` registered for entity `"identity"`. Routed through `OperationDispatcher`; logged automatically by `OperationLogObserver` so undo/redo replay works without bespoke plumbing.

**User-facing operations**:

- `merge_entities(canonical_a, canonical_b)` — snapshot A's row + alias list; rewrite all aliases A→B; delete A from `canonical_entity`. Verifies canonical_b exists before mutating to avoid phantom aliases. Inverse: `restore_canonical_after_merge` carrying the full snapshot.
- `propose_merge(id, kind, evidence_json, created_at)` — INSERT into `proposal_queue` with status='pending'. The `id` is caller-provided to keep replay deterministic. Inverse: `delete_proposal(id)`.
- `accept_proposal(id)` — captures the previous status, then UPDATE to 'accepted'. Inverse: `revert_proposal_status(id, prev_status)` (self-inverse primitive).
- `reject_proposal(id)` — same shape, sets 'rejected'.

**Internal undo primitives** (in `operations()` so the dispatcher routes inverse executions; not user-facing):

- `restore_canonical_after_merge(id, kind, primary_label, created_at, merged_into_id, alias_keys_json)` — re-INSERT canonical, rewrite each captured alias back, including original confidence. Inverse: `merge_entities(id, merged_into_id)` — perfectly symmetric.
- `delete_proposal(id)` — snapshot row, DELETE. Inverse: `restore_proposal(id, kind, evidence_json, status, created_at)` — re-INSERT with original status (handles non-pending deletions correctly).
- `restore_proposal(...)` — re-INSERT a proposal row exactly. Inverse: `delete_proposal(id)`.
- `revert_proposal_status(id, status)` — snapshot current, set new. Self-inverse.

**Files touched**:

- `crates/holon/src/identity/mod.rs` (new) — public surface; re-exports `IdentityProvider`, `ENTITY_NAME`, `SHORT_NAME`.
- `crates/holon/src/identity/provider.rs` (new, ~520 lines) — full provider impl + helpers.
- `crates/holon/src/lib.rs` — `pub mod identity;`.
- `crates/holon/src/di/registration.rs` — wires `IdentityProvider` in both DI paths (Unix `register_core_services` and pre-created-backend `register_core_services_with_backend`), each with `.with_dependency::<DbReady<IdentityTables>>()`. Registered as `dyn OperationProvider` in `register_shared_services` so the dispatcher includes it.
- `crates/holon/tests/identity_operations.rs` (new, ~370 lines) — 6 tokio tests including a proptest with 12 random scenarios.

**Verification**:

- `cargo test -p holon --test identity_operations` — 6/6 green:
  - `merge_entities_rewrites_aliases_and_deletes_a` — alias rewrite + canonical deletion + inverse shape.
  - `merge_then_undo_restores_state_exactly` — full snapshot equality after undo, redo correctness, inverse-of-undo is the original forward op.
  - `merge_with_no_aliases_round_trips` — empty alias snapshot edge case.
  - `propose_merge_then_undo_round_trips` — propose → delete inverse cycle.
  - `accept_proposal_undo_restores_pending_status` — inverse captures the precise previous status.
  - `random_merge_undo_round_trips` (proptest, 12 cases) — for any 2..5 canonicals + 0..8 aliases topology and any merge pair, `merge → undo` round-trips state exactly.
- `cargo test -p holon --test identity_schema_smoke` — 3/3 still green (no regression to step 4.1).
- `cargo check -p holon` clean.
- Architecture tests have 2 pre-existing failures (`no_raw_sql_in_frontends`, `no_underscore_prefixed_params`); none of the flagged files are in `identity/`.

**Exit criteria met**:

- ✅ Tables exist (step 4.1).
- ✅ Three user-facing operations land (`merge_entities`, `propose_merge`, `accept_proposal`/`reject_proposal`); merge has full undo via inverse op.
- ✅ PBT coverage on merge + undo (alias rewrite + delete-and-restore round-trip).
- ✅ Other PBTs unchanged (table starts empty, identity tables don't intersect with block tables).
- ✅ `Architecture/Schema.md` "Entity Identity" section already documents the surface (added in step 4.1).

### Phase 5 — Validation gate, then green-light [DONE 2026-04-26 with caveat]

**Verdict: GREEN-LIGHT**. The three architectural seams are stable; parallel feature work is unblocked. One pre-existing PBT failure remains, in a known org-whitespace-normalization bug class unrelated to any seam — tracked as a follow-up task.

**Checks**:

- ☑ Architecture docs reflect the new seams:
  - `Architecture/Sync.md` — new "FileFormatAdapter (file-backed sync)" subsection above EventBus, citing `OrgFormatAdapter` as first impl and the path for new format adapters.
  - `Architecture/RenderPipeline.md` — new "Widget registry seam" subsection in EntityProfile System, documenting the parse-time source-driven name discovery + interpret-time `builder_registry!` macro.
  - `Architecture/Schema.md` — "Entity Identity" section's "Operations" subsection rewritten to reflect the landed `IdentityProvider` + the user-facing/internal-undo split. "planned, not yet landed" language removed.
- ☑ `TODO.md` cleared of stale architectural items: removed the `* Implement OperationDispatcher` entry (the dispatcher has been live for many releases). Bug entries and code-quality TODOs retained.
- ☑ `LAYOUT_MUTATIONS_ENABLED = true` in `pbt/state_machine.rs` — re-enabled after the Phase 0 ReactiveViewModel fix; not env-var gated. Other env vars in the PBT (`PROPTEST_SEED`, `PBT_WEIGHT_*`, `PBT_MEMORY_MULTIPLIER`, `HOLON_PERF_BUDGET`) are tuning knobs, not test gates. Only `PeerEdit::Delete` remains disabled, with a documented gap (cascading-delete ref-model coverage) tracked separately from this plan.
- ☑ Seam-touching tests all green:
  - `cargo test -p holon --test identity_operations` — 6/6 + 12-case proptest.
  - `cargo test -p holon --test identity_schema_smoke` — 3/3.
  - `cargo test -p holon --lib render_dsl` — 13/13 (incl. new "kanban without registration" test).
  - `cargo check -p holon-frontend` clean.
  - `cargo check -p holon` clean.
- ◐ `cargo test -p holon-integration-tests --test general_e2e_pbt` — 2 of 3 variants pass (`general_e2e_pbt`, `general_e2e_pbt_cross_executor`). `general_e2e_pbt_sql_only` shrinks to a SplitBlock at position=8 of an 8-char content where production stored `"s  uhjdo"` and reference has `"s  uhjd o"` — an org-parser interior-whitespace normalization divergence in the same class as the already-tracked `pbt_bug_c_trailing_whitespace_fix` (which handled the first-line trailing case). **Not a seam regression** — none of Phase 1/2/4 changed the org parser, the reference model's `split_block`, or the assertion path. Tracked as a separate task; does not block the gate verdict because (a) the seam tests above all pass, (b) the divergence is upstream of the seam work, and (c) the user expected this class of PBT failure to remain as known-pending work.

**Green-light: parallel feature work on**:

F1 Obsidian · F3 polish · F5 GPUI editor · F7 search (FTS5 SchemaModule) · F8 kanban · F9 calendar · F10 GQL rendering · F12 parametric styles · Petri-Net incremental syntax (`@` `?` `>` parser, verb dictionary, delegation sub-nets) · Action Watcher feature work · GPUI Android push.

**Open follow-ups (parallelizable, not seam-blocked)**:

- Task #10 — ToggleState target divergence under custom-layout `index.org`.
- Task #11 — Peer-merge block ordering divergence on org file rewrite.
- Task #12 — `general_e2e_pbt_sql_only` SplitBlock content divergence (org parser interior-whitespace normalization).
- Architecture-tests two pre-existing failures (`no_raw_sql_in_frontends`, `no_underscore_prefixed_params`) — unrelated cleanup, not gate-blocking.

## Dependencies

```
Phase 0 [DONE] ─┬─> Phase 1 ─┐
                └─> Phase 2 ─┤
                  Phase 3 ───┼─> Phase 5 (gate) ─> parallel features
                  Phase 4 ───┤
                  Task #7 ───┤  (chord dispatch — separate bug)
                  Task #8 ───┘  (inv16 draggable — separate bug)
```

Phase 0 (signal propagation) blocked 1 and 2 because both touch render-pipeline paths that the cleanup affects. Phases 3 and 4 are independent and can run in parallel with 1 and 2 if capacity allows. Tasks #7 and #8 are pre-existing PBT-surfacing production bugs that block the Phase 5 gate; they can be fixed in parallel with Phases 1–4.

## Explicitly deferred

Not blocking parallel feature work — additive layers that can land on top of stable seams later:

- **AI services trait + Trust Ladder schema** — design the `trust_ladder` table now if convenient, postpone the trait. Wraps `OperationDispatcher` later without breaking PBTs.
- **FTS5 / embeddings (F7)** — `SchemaModule` registry is already the right seam. Pure addition.
- **Self DT + telemetry collector** — Phase 6 vision.
- **Three-mode UI controller** (Capture / Orient / Flow) — cross-cutting risk; watch but don't block. If mode-aware queries route through one ViewModel from day one, the formal `ModeController` can be retrofitted.
- **Conflict resolution beyond LWW** — needs the AI service layer to land first.
- **Browser plugin / WASM sandboxing / SOP extraction** — vision but post-first-release.

## Out of scope for this plan

- Feature implementation (F1–F12 themselves)
- Petri-Net language extensions beyond what the architecture already supports
- Mobile (GPUI on Android) input/IME story — separate plan
- Sharing / collaboration permissions — separate plan, depends on Phase 4

## Success criteria for the plan as a whole

After Phase 5 the user can spin up two or three features in parallel without coordinating refactors and without expecting major PBT churn for at least one release cycle. The architectural docs match the code. The PBTs are the gate, not the bottleneck.

## Tracking

Phases are tracked as TaskCreate tasks 1–6 (`Phase 0` through `Phase 5 (gate)`). Use `TaskList` to see live status.
