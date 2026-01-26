# ADR Audit — are all 16 ADRs still honored by the code? (2026-07-06)

Read-only architectural audit of every ADR in `docs/adr/` against the current
tree at HEAD, after the "Petri-net based engine refactor" merge (`30127e8e12`).
Method: for each ADR, extracted the concrete decision + consequences, then
verified each checkable claim (named types, traits, crates, invariants, data
layouts) against code via `ast-outline` + targeted reads. Every verdict below
carries `file:line` evidence.

## Verdict table (VIOLATED / STALE / SUPERSEDED first)

| ADR | Verdict | Key evidence (file:line) | Recommended action |
|---|---|---|---|
| **0002** trait-based unified type system | **STALE** (never built) | Status still "Proposed" (2025-11-02) `0002:767-769`; zero hits for `Completable`/`Prioritizable`/`Schedulable`/`Hierarchical`, `BlockView`, `Blocklike`, `BlockAdapter`, `NormalizedPriority`, `TodoistTask`, `JiraIssue` anywhere in `crates/`. `BlockContent` is a *different* enum (`crates/holon-api/src/block.rs:24-41`). | Mark `Superseded By: 0004/0006 + generic MCP sidecar`. Name-collides with 0012 "capability traits" → actively misleading. |
| **BLOCK_LORODOC_ARCHITECTURE.md** | **SUPERSEDED** (correct, self-labelled) | Header already says Superseded by 0003 (`:3-14`); no per-block content LoroDocs / `content_doc_id` / `block_snapshots` table exist; content is nested `LoroText` per 0003 (`crates/holon-loro/src/loro_backend.rs:34`). | None. Optionally move to a `rejected/` folder / rename `0003a-…` so it isn't mistaken for an accepted numbered ADR. |
| **0007** wiring manifest for PBT subsets | **SUPERSEDED** (banner accurate; core type lives on) | ADR carries a 2026-07-05 SUPERSEDED banner (`0007:1-8`) — verified. `Wiring`/`RequiredWiring`/`validate()` alive (`crates/holon-pbt-core/src/wiring.rs:86,258,227`), consumed by the keystone `any_valid_wiring()` (`wide_e2e.rs:645`). Dead: `pbt_slice!`/`component_pbt!` macros (zero hits); production-DI `Wiring` consumption never happened. | Keep banner; add one line: `Wiring` survives as keystone input, production-DI half dropped. |
| **0001** hybrid sync architecture | **PARTIALLY-HONORED** (intent survives, most mechanics never built or replaced) | Two-model split survives (`crates/holon-loro/src/loro_backend.rs:1-11`, `entity_uri.rs:11-15`). But the ADR's `DataSource<T>` CRUD trait is actually read-only (`crates/holon-core/src/traits.rs:452-458`); no `ReconciliationEngine`, no Conflict-Resolver UI, no per-provider `SyncStrategy` of the ADR's shape; `PendingSync`/`Synced`/`Cancelled` all "(future use)" (`operation_log.rs:15-24`). Third-party sync rebuilt as generic MCP+YAML sidecars (`holon-mcp-client`, `docs/integrations/todoist.yaml`). Turso role widened from "3rd-party shadow cache" to projection-of-owned-data (`holon-turso` matviews/CDC + consolidator). Still says "Superseded By: None". | Mark "Partially superseded"; strike §2b/2e/2f/2g as never-implemented; point to 0002/0004 + MCP sidecar directive. |
| **0008** sort_key serialization migration | **PARTIALLY-HONORED** (decision holds; cited verification test does not exist) | Decision honored: domain `Block` has no `sort_key` (`block.rs:273-303`), Loro authority = fractional index (`loro_backend.rs:910-913`), Turso column retained no-migration (`holon-turso/sql/schema/blocks.sql`). BUT the promised test `legacy_snapshot_orders_from_fractional_index_not_domain_sort_key` exists nowhere (`git log -S` shows the string only ever landed in the ADR), and the cited path `crates/holon/src/api/loro_backend.rs` doesn't exist (it's `crates/holon-loro/…`). | Write the promised legacy-snapshot test, or rewrite the Verification section to cite what actually guards it + fix the stale path. |
| **0009** component-subset PBTs + bisection | **PARTIALLY-HONORED** (core honored, entry-point shape superseded, doc rot) | `ComponentSet`/`Projection`/lattice ops real (`component_set.rs:25-31,243,288`); bisection real (`bisect.rs:39,64,89`, `bisection_pbt.rs`); operates on recorded `Vec<E2ETransition>` not seeds. But `run_component_pbt(set, SeedSource)` + per-combo entry points never built — replaced by `ComposedSut<WideE2E>` + `any_valid_wiring()`. Stale `E2ESut` doc comments (`bisect_driver.rs:7`, `local_caps.rs:29-54`) — the type is deleted. | Add partial-supersession note; fix dead `E2ESut` doc comments. |
| **0011** filesystem port trait | **PARTIALLY-HONORED** (honored in declared scope; org-vault bypasses outside it) | Ports exist + org-sync path wired via `Arc<dyn FileSystem>` (`holon-filesystem/src/fs_port.rs:36`, `holon-orgmode/src/*`). ADR scope is confined to `holon-orgmode`, so "ALL fs through port" was never the decision. Bypasses that write **org-vault** data raw: `frontends/gpui/src/mobile.rs:80-96` (seeds `index.org`/`Journals.org` via `std::fs::write` — strongest), `editor_view.rs:1290,1300` (attachments), `frontends/mcp/src/tools.rs:1375`, `holon-app/src/wiring.rs:199` (ADR notes say holon-app should stay org-fs-free), `holon-orgmode/src/di.rs:252` (`canonicalize` + `unwrap_or` swallows error). | Route `mobile.rs` seeding through existing `seed_assets` port; decide/document attachments + MCP writes; widen or clarify ADR scope. |
| **0012** reference-model capability contract | **PARTIALLY-HONORED** (contract alive & load-bearing; §4 mechanics + cited artifacts superseded) | Contract surface grew (41 `Ref*/Sut*` traits `capabilities.rs`); cap-bounds still drive invariants (`composed/correspondences.rs:14-18`). But §4 registry gate is **deleted** (`invariants/registry.rs:1-9` — composed catalog is sole manifest now); `E2ESut` deleted, `sut_capabilities.rs` gone → `local_caps.rs` + `CapMap` dyn-composition (weakens the "no `dyn`" claim, `reference_capabilities.rs:982`); every §1 line number stale. | Amend (don't supersede): rewrite §4 around composed catalog + `Correspondence`/`CapMap`; refresh all artifact refs + line numbers. |
| **0013** test-support boundary | **HONORED** (two documented-drift items) | `pbt_infrastructure` gated `any(test, feature="testing")` (`crates/holon/src/api/mod.rs:26-27`); `storage`/`di` `test_helpers` gated. "proptest in prod builds" concern RESOLVED — proptest is dev/optional-only in every prod crate; the 3 crates carrying it in `[dependencies]` are test-support crates reached only via dev-deps/`pbt` feature. Drift: (a) `test-helpers` feature is no longer "empty" — now `["dep:tempfile","holon-loro/test-helpers","testing"]`; (b) the "4th env seam ⇒ migrate to HolonConfig" trigger is now HIT (4 `HOLON_PBT_*` seams incl. new `HOLON_PBT_QUIESCENCE_TIMEOUT_MS` at `user_driver.rs:1086`). | Migrate the 4 env seams to `HolonConfig` per the ADR's own trigger, or sanction the 4th; refresh the feature-contract paragraph. |
| **0003** all-in-LoroTree | **HONORED** (two metadata details evolved) | Single global LoroTree in one LoroDoc (`loro_backend.rs:1-11`, `TREE_NAME="blocks":54`); fork-and-prune + mounts + iroh sync all present as named (`shared_tree.rs:116-437`, `iroh_sync_adapter.rs`). Evolved: `properties` JSON blob → nested per-property `LoroMap` (H3, `PROPERTIES_MAP`); `is_document: bool` → `"Page"` tag (`block.rs:284-285`). | Keep Accepted; add an "Amendments" note (properties_map, Page tag, xref 0014). |
| **0004** domain/adapter/actor split | **HONORED** (one consequence unrealized) | `Wiring{storage_adapters,sync_adapters,actors}` (`wiring.rs:85-90`); god-struct `ReferenceState` factored into tier-1 + per-actor fragments (`reference_domain_state.rs`, `*_actor_state.rs`); ordering authority per adapter (`wiring.rs:77-82,216`); Turso-less Holon expressible (`holon-app/src/no_turso.rs`). Gap: production binaries still don't consume a `Wiring` (PBT-only), already disclosed in 0012. | Flip "Proposed"→Accepted; note production-DI Wiring still open. |
| **0005** children-as-ordered-list | **HONORED** (loose ends) | Domain `Block` has no `sort_key` (`block.rs:273`); `children_of` is the ordering primitive (`memory_backend.rs:112`, `loro_backend.rs:805`, `block_query.rs:55-71`); grouping is one fn `ContentType::sibling_order_group()` (`types.rs:174`) via **stable** `sort_by_key` in both renderers; (lamport,peer-id) tie-break complies (`loro_backend.rs:900-927`). Loose ends: `children_of_window` (a written MUST) **never implemented** (zero hits); `assign_reference_sequences_canonical` undead in test infra; status still "Proposed"; open `inv-org-render-fixed-point` sibling-order flake touches this claim. | Flip to Accepted; implement `children_of_window` or drop the windowed-read MUST. |
| **0006** actor terminology + MCP dual role | **HONORED** | `enum Actor{UI,MCPServer,ActionEngine}` (`wiring.rs:59-63`); `UIActorState`/`MCPServerActorState`/`ActionActorState` named exactly; MCP server=Actor, MCP client=Tier-2b `SyncAdapter`; both claimed weakness-resolutions real (`render_dsl.rs`/`action_dsl.rs` in domain; `UITabState`+`UIUserState` split). Minor: concrete `GCalSyncAdapter`/`GMailSyncAdapter` structs never built (manifest variants + YAML sidecar instead). | Keep; one line: sync adapters materialized as manifest variants + generic YAML sidecar, not per-integration structs. |
| **0010** focus authority in-memory signal | **HONORED** (prime suspect — clean) | `editor_cursor` table + `current_editor_focus` matview **removed** at schema level (`holon-turso/sql/schema/navigation.sql:17-18`); no prod `INSERT editor_cursor`; authority = `UiState.focused_block: Mutable<…>` (`reactive.rs:953`); initial-caret carrier = `pending_caret_seed` (`reactive.rs:966-972`); backend follow-up reaches signal in-process, no CDC (`traits.rs:1117,1282`). The lone `current_editor_focus` string is an inert SQL-parser unit test (`util.rs:141`). Petri "focus" is an unrelated task-energy namesake. | Flip "Proposed"→Accepted/Implemented; status line understates it. |
| **0014** doc-scheme retirement | **HONORED** | `rg '"doc:'` matches only frozen turso repros, the negative tooth (`link_parser.rs:326-329`), and one sanctioned `#[cfg(test)]` fixture (`traits.rs:1959`). `set_is_document`, the `roots` PRQL relation, `doc:` acceptance arms — all zero hits. Exactly the residue set the ADR predicted. | None. |
| **0015** computed placement / curated state | **HONORED-as-proposal** (not yet implemented, by design) | Status "Proposed (2026-07-06; not implemented)", P2 explicitly GATED. Zero hits for `DisplayPlacement`/`CuratedState`/occurrence-path types — correct, nothing jumped the gate. Its current-state citations are accurate (`focused_block` still bare-`EntityUri`-keyed). Companion plan `docs/Proposals/display-placement-implementation-plan.md`. | None now; when P2 starts, the bit-identity gate invariant + focus-rekeying ADR must land first per the ADR's own gate. |

## The Petri refactor — framing correction

The task prompt assumed "the Petri-net engine refactor removed old reactive
types." Verification shows these are **two independent changes bundled in the
octopus merge `30127e8e12`**:

1. **Petri engine** = task-ranking/simulation, NOT the sync/reactive engine.
   - `crates/holon-engine/src/lib.rs:1-30` — standalone, holon-independent
     Petri-net engine (YAML nets, Rhai guards, WSJF ranking, what-if). Traits:
     `TokenState`, `TransitionDef`, `NetDef`, `Marking`, `Engine`,
     `RankedTransition` (WSJF = Δobjective / duration).
   - `crates/holon-petri/src/lib.rs:1-28` — materialization layer: tokens =
     entities, transitions = tasks, objective scored via prototypal inheritance
     (`>` sequential dep, `@[[Person]]:` delegation, `?` knowledge token).
2. **Frontend reactive cleanup** (separate) removed `AppState`,
   `spawn_ui_listener`, `CdcState`, `BlockWatchRegistry`, `ReactiveViewKind`,
   `RenderPipeline.widget_states` — replaced by a single `ReactiveEngine`
   (`crates/holon-frontend/src/reactive.rs:1165`, module doc: "Replaces
   CdcAccumulator + BlockWatchRegistry + AppState") and persistent-node
   `ReactiveViewModel` (`reactive_view_model.rs:302-304`). Backend
   `CdcAccumulator`/`watch_ui` survive (`holon-api/src/reactive.rs`,
   `holon/src/api/ui_watcher.rs:48`) — only the frontend consumption layer was
   rebuilt.

**Supersession scan of removed types:** NO removed type name
(`AppState|CdcState|BlockWatchRegistry|ReactiveViewKind|spawn_ui_listener|…`)
appears in ANY `docs/adr/*.md`. **No ADR needs a "superseded" marker on account
of the reactive-type removal.** (Stale mentions live only outside docs/adr — in
`docs/Architecture.md`, `docs/Testing/*`, `docs/Archive/*`.) ADRs 0009/0012/0013
reference *surviving* reactive types (`ReactiveEngine`/`ReactiveViewModel`/
`ReactiveEngineDriver`) but were written against the snapshot-based semantics —
worth a light "verified against persistent-node rewrite" note, not supersession.

## Missing ADRs — significant decisions with NO ADR coverage

| Decision | Anchor | Coverage today |
|---|---|---|
| **Petri-net task-ranking engine** (the merge's headline feature) | `holon-engine/src/lib.rs`, `holon-petri/src/lib.rs` | **No ADR.** Promotable material: `docs/Vision/PetriNet.md` §Design Decisions + `docs/Architecture/Engine.md`. Strongest ADR gap. |
| **ReactiveEngine / persistent-node ReactiveViewModel rewrite** | `holon-frontend/src/reactive.rs:1165`, `reactive_view_model.rs:304` | **No ADR** (docs/Architecture/UI.md documents it as implemented). Strong candidate. |
| **Correspondence registry** (declarative test-wiring `Correspondence::wire`) | `holon-integration-tests/src/pbt/correspondence.rs:126,136`, `holon-macros/src/capability_pair.rs` | **No ADR, no doc at all** — only session memory. |
| **CapMap / capability DI** (insert-only per setup) | `holon-pbt-core/src/composition.rs:107` | **No ADR** (0012 covers cap *traits*, 0007 the manifest; CapMap itself uncovered). |
| **Turso IVM / chained-matview bet** (incl. known hangs) | `holon-turso/src/matview_manager.rs:203` | **No dedicated ADR** (0001 has a one-liner). `docs/Architecture/Sync.md` documents it, but the *bet* on chained matviews has no decision record. |
| **WatchEnvelope / watch_snapshot_stream focus keystone** | `holon-frontend/src/view_model.rs:46`, `reactive.rs:1319` | **No ADR** — neither name appears in docs/. 0010 is the adjacent prior decision. |
| **Command/operation sourcing** (persist for undo now, offline later) | `holon/src/core/operation_log.rs:22`, `operation_dispatcher.rs:31` | **No ADR** — only memory. |
| **Org round-trip fixed-point invariant** (`disk == rendered`) | `holon-pbt-core/src/capabilities.rs:1536-1544` | **No ADR.** |

## Cross-cutting recommendations

1. **Status-line sweep**: 0004, 0005, 0006, 0010 are all fully executed but
   still read "Proposed". Flip to Accepted.
2. **Write the Petri ADR** — the merge's headline decision has zero ADR
   coverage; distill from `Engine.md` + `PetriNet.md`.
3. **0002 → Superseded** (never built, and its "capability traits" name now
   collides with 0012).
4. **0008 test claim is false** — write the promised test or fix the ADR.
5. **0013 trigger is hit** — migrate the 4 `HOLON_PBT_*` env seams to
   `HolonConfig`.
6. **0011 org-vault bypasses** — `mobile.rs` raw `std::fs::write` of seed org
   files is the strongest single violation; route through `seed_assets`.
7. **Doc-rot cleanup**: stale `E2ESut` doc comments in `bisect_driver.rs` /
   `local_caps.rs` (type deleted).

**Bottom line:** no ADR is outright VIOLATED by the code. The risks are
documentation drift (0001/0008/0012 cite artifacts/tests that no longer exist)
and undocumented decisions (Petri engine, ReactiveEngine rewrite, correspondence
registry) — the code moved ahead of the record, not against it.
