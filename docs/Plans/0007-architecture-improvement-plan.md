# 0007 — Architecture Improvement Plan

**Status: accepted, Phase 0 in progress (2026-06-11).**

Source: full read of all Vision/Architecture docs + three parallel code audits
(adherence, intrinsic weaknesses, undocumented architecture) on 2026-06-11,
reconciled with Martin's decisions:

- External systems integrate via the MCP-client + YAML-integration path (the
  deleted `holon-todoist` crate is not coming back).
- AI (Watcher/Integrator/Guide) will NOT be hard-coded services — AI agents are
  Petri-net tokens consumed by transitions (e.g. "Research"); the three roles
  ship as default, user-overridable declarative configuration.
- Capture/Orient/Flow ship as default configuration, not as a code-level mode
  concept.
- GPUI is the primary frontend (mobile via gpui-mobile, layout work pending
  desktop dogfood); Dioxus web is a lower-priority prototype; Flutter is out.
- The with/without Turso/Loro/Org matrix is a deliberate decoupling +
  PBT-testability strategy, not a degraded mode to be removed.
- Root `ARCHITECTURE.md` / `ARCHITECTURE_PRINCIPLES.md` are deleted;
  `docs/Architecture.md` + `docs/Architecture/*` are the single source of truth.

The audits' core verdict: the substrate architecture is sound; the dominant
intrinsic risk has one root cause — **four stores of the same block state, two
live write implementations, and string-mediated identity between them** — and
the dominant documentation risk was duplicated docs drifting independently.

---

## Phase 0 — Truth restoration (docs + cheap gates) — IN PROGRESS

Goal: docs describe the system that exists; documented enforcement claims are
true; the cheapest fail-loud violations are fixed.

| # | Item | Where | Status |
|---|------|-------|--------|
| 0.1 | Delete root ARCHITECTURE.md / ARCHITECTURE_PRINCIPLES.md, fix pointers (README, AGENTS, CLAUDE) | repo root | done |
| 0.2 | Rewrite Todoist/integration docs to the MCP-registry reality | docs/Architecture/Integrations.md, Operations.md, Architecture.md | done |
| 0.3 | Doc surgery: frontend status (GPUI primary), AI-as-PetriNet-config reframing, Three-Modes-as-config note, CollaborativeDoc removal, real component names (LoroBlockOperations, LoroProjection, BlockConsolidator), fluxdi (not ferrous-di), crate inventory, cell-backing + archlint-rule status banners, ADR 0011 location note | docs/Architecture/*, docs/adr/0011 | in progress (agent) |
| 0.4 | Implement the `sole_block_writer` archlint gate the docs already claim exists | archlint/smells/ | in progress (agent) |
| 0.5 | Fail-loud MCP integration loading: YAML parse failures and `${VAR}` resolution failures error instead of warn-and-skip; distinguish "invalid config" (error) from "explicitly unconfigured" (disclosed skip) | crates/holon-mcp-client/src/integration_config.rs, crates/holon-app/src/mcp_integrations.rs | in progress (agent) |
| 0.6 | Name-keyed integration registry (replace positional index that can misroute ops when a connection fails) | crates/holon-app/src/mcp_integrations.rs | in progress (agent) |
| 0.7 | Un-invert `holon-api → holon-engine` edge: extract `CompiledExpr` to a leaf `holon-expr` crate, re-export from old paths | crates/holon-expr (new), holon-api, holon-engine | in progress (agent) |
| 0.8 | ADR 0012: reference-model capability contract (the PBT lattice — largest undocumented subsystem) | docs/adr/0012 | in progress (agent) |

Exit criteria: `archlint --all` green incl. new gate; adherence-audit findings
1–3, 6, 8–10, 14 closed; `cargo check -p holon-mcp-client -p holon-api
-p holon-engine` green (full workspace blocked by unrelated WIP).

## Phase 1 — Single-writer completion (kills the dominant bug class)

Goal: finish Replication.md invariants 2–5 so SQL/Loro/org divergence becomes
unrepresentable instead of PBT-discoverable.

1. **Sole Turso writer** (Replication.md §11 step 3): the consolidated feed
   becomes the only runtime writer of `block_raw`; SqlOnly mode keeps its
   writer but the two can never be live simultaneously. Tighten
   `sole_block_writer` excludes as paths are removed.
2. **Authority-first delete**: `block.delete` currently leaves a Loro orphan
   (generic raw-SQL delete never touches Loro). Route delete through the
   intent path (glittery-gliding-rossum Phases 4/5).
3. **Total, verbatim fractional-index projection** — every owned block's fi
   reaches the sort_key column; no second keyspace
   (Replication.md §5; the "sort_key stays A0" class).
4. **Finish or descope the cell backings**: `LoroMetaCellBacking<T>`,
   `LoroTreeParent/PositionCellBacking`, `LwwScalarBacking<T>` are documented
   but unimplemented; either land them (Cells plan Phase 2) and migrate the
   `write_field` scalar/tree carve-outs through cells, or update the plan.
   Add the remaining documented archlint cell gates as fields migrate.
5. **Intent channel carries `after_sibling`, never order keys** — wire-type
   check (parse-don't-validate on the bus type).

Verification: `just pbt` slices (sql_only, Full/Loro, no-Turso, with/without
Org) green; the divergence-family fixtures in /tmp captures replay clean.

## Phase 2 — Identity hardening

Goal: stringly-typed IDs stop being a bug source.

1. Ratchet `EntityUri::from_raw` (111 non-test call sites → 0) via the
   existing archlint entity_uri smell; thread `EntityUri` end-to-end (typed at
   storage/serde boundaries, parse at the org/Loro/SQL edges only).
2. Fix the 4 silent `std::fs::canonicalize(&p).unwrap_or(p)` fallbacks
   (holon-orgmode/src/di.rs ×3, holon/src/sync/loro_module.rs) — the
   `/var` vs `/private/var` symlink class; route through the ADR 0011
   FileSystem port.
3. Document EntityUri scheme conventions + `new` vs `from_raw` contract in
   docs/Reference (small section; the from_raw heuristic already caused one
   prod data fork).

## Phase 2.5 — Subsystem coupling: Turso / Loro / Org / Markdown — DONE (2026-06-11)

Goal: the four storage/sync subsystems are supposed to be architecturally
decoupled (independently toggleable, interacting only through neutral
abstractions: capability traits, cells, `BlockOperations`,
`FileFormatAdapter`, CDC/`LiveData`, `EventOrigin`). A 2026-06-11 audit of
cross-mentions found the boundary mostly holds, with these exceptions.

### Hard couplings (type/Cargo-level — fix by trait extraction)

| # | Coupling | Where | Resolution (2026-06-11) |
|---|----------|-------|-------------------------|
| C1 | Org → Loro: `use holon::api::loro_backend::LoroBackend` in the org sync adapters | was `crates/holon-orgmode/src/loro_sync_adapters.rs:22` | **DONE** — the three Loro adapters moved verbatim to `crates/holon-app/src/loro_org_sync.rs`; holon-orgmode has zero Loro mentions left |
| C2 | Org → Loro: `spawn_loro_org_sync(backend: Arc<LoroBackend>, …)` | was `crates/holon-orgmode/src/di.rs:1525` | **DONE** — moved to `holon_app::spawn_loro_org_sync`; `run_file_sync_controller`/`OrgRerender`/`LoroAliasRegistrar.doc_store` publicized as the seam |
| C3 | Org → Turso: `LiveDocumentManager::new(.., Arc<RwLock<TursoBackend>>)` | was `crates/holon-orgmode/src/di.rs:494` | **DONE** — constructor takes the storage-neutral `holon::storage::DbHandle` (it only ever called `backend.handle()`); `TursoBackendProvider` resolution dropped from holon-orgmode entirely, ALLOW(turso) marker removed |
| C4 | Loro-sync → Turso: `DbHandle` import in the projection | was `crates/holon/src/sync/loro_sync_controller.rs:42` (`TursoSinkReader`) | **DONE** — `SinkReader` trait stays in the sync layer; `TursoSinkReader` moved to `crates/holon/src/storage/turso_sink_reader.rs` (NOT holon-turso: the trait lives upstream of it; NOT holon-app: `loro_module.rs` wiring already resolves `DbHandleProvider` and is the designated assembly point) |
| C5 | Loro-sync → Turso: `TursoCommandLog` wraps `DbHandle` | was `crates/holon/src/sync/turso_command_log.rs:10` | **DONE (deleted)** — `TursoCommandLog` and the `CommandLog` trait were dead code (trait impl existed, nothing constructed or wired it). Removed per refactor-completely; recover from VCS when undo/redo persistence lands |
| C6 | Sync → Turso: `LinkEventSubscriber` imports `DbHandle` | was `crates/holon/src/sync/link_event_subscriber.rs:14` | **DONE** — `BlockLinkIndexer` trait in the sync layer (link *extraction* stays there); `TursoBlockLinkIndexer` in `crates/holon/src/storage/turso_block_link_indexer.rs`, wired in `event_infra_module.rs` |
| C7 | Sync → Turso: `LiveData<T>` consumes Turso's `RowChange` | was `crates/holon/src/sync/live_data.rs:16` | **DONE** — `apply_changes` takes `Vec<Change<StorageEntity>>`; `subscribe` is generic over `Stream<Item = BatchWithMetadata<C>> where C: Into<Change<StorageEntity>>`; holon-turso provides `From<RowChange> for ChangeData` (drops `relation_name`). Zero `RowChange` mentions left in `crates/holon/src/sync/` |

Caveat on C4–C7: the consolidation layer (`crates/holon/src/sync/`) is the
*designated* place where Loro meets Turso, so these are coupling-direction
smells rather than violations — the projection may know "a SQL sink" exists,
but importing Turso's concrete types means the layer can't be reused for a
different projection target. Trait seams (some already present) are the fix;
priority below C1–C3.

### Soft couplings (acceptable or rename-only)

- `EventOrigin::{Loro, Org, Todoist, Ui}` naming subsystems is **by design**
  (provenance tagging) — justified, keep.
- DI provider trait names leak the backend (`TursoBackendProvider`,
  `DbHandleProvider` resolved from holon-orgmode/di.rs:915,1046) — RESOLVED
  differently: holon-orgmode no longer resolves `TursoBackendProvider` at all
  (C3); `DbHandleProvider`/`DbHandle` is already storage-neutral naming and
  stays as the SQL-mode handle seam.
- ~6 comments/log strings in org/markdown code narrate Loro/Turso internals
  (e.g. block_params.rs:127 explaining `on_loro_changed`) — DONE for the
  worst offenders (block_params sort_key note + 4 `LoroSyncController`-naming
  comments in file_sync_controller.rs, now "the consolidator"); mode-semantics
  comments that legitimately discuss Loro-vs-SqlOnly behavior remain.
- Markdown → Org: none found at type level — `holon-markdown` only implements
  `FileFormatAdapter`; the adapter seam is working as designed. (The
  controller is still *named* `OrgSyncController` while serving both formats —
  rename to `FileSyncController` is cosmetic, queue with C1.) — DONE: renamed
  repo-wide (type, module file `file_sync_controller.rs`,
  `run_file_sync_controller`, docs).

### Enforcement gaps found (and partially fixed 2026-06-11)

1. **archlint glob hole (FIXED)**: `matches_glob` used `fnmatch`, where
   `src/**/*.rs` requires ≥1 directory after `src/` — every top-level file in
   `crates/holon-orgmode/src/` (di.rs, loro_sync_adapters.rs, …) and
   `crates/holon-frontend/src/` was silently exempt from the loro/turso/
   platform/frontend-storage-backend boundary smells. Fixed in
   `archlint/archlint.py::matches_glob` (`**/` now also matches zero
   directories). This immediately surfaced C3 (now ALLOW-marked) and one
   doc-comment false positive in holon-frontend (ALLOW-marked).
2. **Pattern gap (FIXED 2026-06-11)**: the `loro`/`turso` smells now also match
   holon-mediated coupling (`loro_backend::|\bLoroBackend\b` and
   `\bTursoBackend`). `holon::storage` as a whole is deliberately NOT matched:
   `holon::storage::DbHandle` is the sanctioned storage-neutral handle seam
   (C3) and `BLOCK_READ/WRITE_TABLE`/schema-module re-exports are SQL-mode
   wiring; the concrete-backend tokens are what ratchet shut. archlint --all
   has zero loro/turso findings.
3. The `holon-orgmode → holon` Cargo dependency is itself the enabling edge
   for C1–C3 (and for the holon-frontend native leak, Phase 3.1). Removing it
   once the seam traits exist is the structural fix that makes this whole
   class unrepresentable.

### Known reds unmasked by Phase 2.5 (pre-existing, NOT caused by it)

Restoring workspace compilation (the holon lib + holon-mcp + test targets were
compile-broken on main from the StorageEntity Arc<str> migration × block-sync
landing collateral) made these previously-uncompilable tests visible again:

- `holon-orgmode::sync_controller_mutation_pbt` — 3 red:
  - `test_sync_block_change_to_file`: after a parse round-trip, top-level
    blocks carry `parent_id = file:test.org` (path-derived) while renders root
    at the `block:<uuid>` doc id, so `render_entitys` emits a header-only file.
    Parser document-identity / `#+ID:` adoption contract drift — belongs to
    the identity (Phase 2) / block-sync work.
  - `ordering_replay_{calls_place_for_misaligned_block,skips_place_when_order_matches}`:
    block-sync's new `on_file_changed` wait ("new blocks must appear in
    `ordering.children()`") times out against the tests' static/empty
    `children()` stubs — stale stub contract, same landing.
- `holon-api render_eval::tests::test_state_display` — known pre-existing red
  (stage10 notes).

## Phase 3 — Crate hygiene & boundaries

Goal: the dependency graph matches the documented layering on all targets.

1. **holon-frontend native `holon` dep removal**: the only native usages are
   `parse_org_file` (default-asset seeding) and `FileWatcherReadySignal` via
   holon-orgmode — move behind a small holon-api trait or into holon-app
   wiring. Restores "ViewModel layer has no holon dep" on native, not just wasm.
2. **holon-filesystem split**: legacy `directory.rs` DataSource drags a
   `holon` dep into the ADR 0011 port crate — move it out (holon-app or its
   own crate) so the port is a true leaf.
3. **Split the `holon` god crate** (48k LoC, 481 pub items): extract
   `holon-loro` (loro_backend + share_backend + sync controllers),
   `holon-profiles` (entity_profile), `holon-petri-bridge` (petri.rs);
   `holon` keeps composition. Do this AFTER Phase 1 so the extraction moves
   settled code.
4. **Test-support boundary**: move `user_driver.rs`, `headless_editor_mirror.rs`,
   `e2e_test_helpers.rs`, `pbt_infrastructure.rs` behind a feature or into a
   `holon-test-support` crate; replace ad-hoc `HOLON_PBT_*` env switches in
   prod paths with config-file flags where they alter behavior. (Or: document
   "test seams are prod API" as a deliberate ADR — decide, don't drift.)
5. **Async DI**: make factories `Provider::root_async` throughout and delete
   `run_async_in_sync` / `block_in_place` arms (the recurring
   tokio-runtime-mismatch deadlock class). — **DONE (2026-06-12)**: the only
   production `block_in_place` lived in `register_core_services`, which was
   dead code (zero callers) — deleted along with `run_async_in_sync` and sync
   `create_queryable_cache`. The live DI path already follows the converged
   pattern (async boundary acquisition, sync capture factories,
   `Provider::root_async`); documented in `docs/Architecture/Principles.md`.
   Remaining pure-sync `Provider::root` factories deliberately left as-is (no
   deadlock value in converting). `mcp_vtable`'s `block_in_place` is a
   deliberate Turso-FFI boundary, kept.
6. Relocate the ~12k lines of in-src `#[cfg(test)]`/repro modules from
   `crates/holon/src` to `tests/` or a repro crate. — **DONE (2026-06-12)**:
   19 dedicated test/repro files moved to 4 integration-test binaries under
   `crates/holon/tests/` (`api_suite`, `sync_suite`, `turso_storage_repros`,
   `turso_storage_pbt`); `storage::test_helpers` regated behind
   `test-helpers` feature; 2 orphan repro files (never compiled since
   `b8176cc51a`, API-rotted) deleted. Inline `#[cfg(test)]` modules stay in
   src by design.

## Phase 4 — Robustness & freshness

1. **MCP integration freshness**: optional poll interval per integration (sync
   currently runs only at startup + on MCP notifications, which Todoist's
   server may never send); verify notification behavior empirically.
2. **Per-integration MatviewHook** (today only the first integration's sync
   engine gets `on_fdw_primed`).
3. **IVM disclosed fallback**: per-matview full-requery path so a Turso IVM
   bug degrades visibly instead of corrupting (keep the turso-sql-replay
   repro discipline).
4. Env-var/config registry doc: one reference table for the ~55 `HOLON_*`
   vars + `HolonConfig`/holon.toml layering; delete one-off debug vars that
   should be `RUST_LOG` targets.

## Phase 5 — Vision enablers (architecture meets product)

1. **AI-as-tokens surface**: define how an AI agent token + transition
   (e.g. "Research") is declared in configuration (type definitions +
   prototype blocks already provide the substrate); Watcher/Integrator/Guide
   land as default nets, not code. Trust levels attach to transitions
   (Vision/PetriNet.md §AI).
2. **Modes as default config**: Capture/Orient/Flow as shipped layout/profile
   configuration over the existing entity-profile + layout system.
3. **Frontend tiering execution**: gpui-mobile layout pass (after desktop
   dogfood gate); decide on extracting per-frontend feature branches from the
   jj mega-merge; archive flutter.
4. **Agent-provenance demo** (the killer demo per the landscape review):
   agent-authored blocks carry `:source: tool-call-id`, re-executable source
   blocks, and a live "what did the agent change in the last hour" PRQL view —
   all three are one configuration + small features away on the existing
   substrate.

---

## Sequencing rationale

Phase 0 is pure truth-restoration and is being landed by parallel agents now.
Phase 1 before Phase 3: extracting crates around a dual write path would
ossify it; finish single-writer first, then split settled code. Phase 2 can
interleave with Phase 1 (independent files). Phases 4–5 ride on green PBT
gates from Phase 1.

Each phase's exit gate is the existing test matrix: `archlint --all`,
`cargo nextest` workspace, and the PBT slices (sql_only / Full / no-Turso /
±Org), per DEVELOPMENT.md.
