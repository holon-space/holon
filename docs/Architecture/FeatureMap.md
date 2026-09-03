# Holon Feature Map

GENERATED — edit [`featuremap.yaml`](featuremap.yaml), then run `python3 scripts/featuremap.py generate`. `python3 scripts/featuremap.py check` fails when this file and the pin sources disagree.

The one-page answer to *"which features could this change impact?"*. Rows are user-visible features and cross-cutting aspects; columns say where each one is **pinned** (what test fails if you break it), **ruled** (which decision governs it), and **entered** (the one file to open first).

## How to use this

- **Before a refactoring discussion**, scan the area tables for anything your
  change touches. A row with a pin is safe to change boldly — the keystone will
  tell you. A row in [Unpinned features](#unpinned-features) is one you must
  reason about by hand.
- **Pins are the currency.** A "Pinned by" cell names either a keystone
  *transition* (what drives the feature) or an *invariant id* (what observes it).
  Both are verbatim from code. Grep the name and you land on the test.
- **This is a map of pointers, not a spec.** Every cell should be one clause.
  If you want the detail, follow the link.

## How to keep it current

- A discussion that adds a feature adds a row to
  [`featuremap.yaml`](featuremap.yaml) and regenerates.
- A new transition or invariant needs no edit to be visible: it appears under
  [Unclaimed by any row](#unclaimed-by-any-row) until a row claims it.
- A row that moves out of [Unpinned features](#unpinned-features) is a small
  celebration — note it in the PR that added the pin.
- **Stale links are bugs, and here they are fatal.** Generation refuses a row
  naming a path, a link target, a transition, an invariant id, or a known-red
  key that no longer resolves.

## Legend

| Column | What it holds |
|---|---|
| **Pinned by** | Keystone transition struct names (`SplitBlock`) and/or invariant ids (`inv-no-orphan-blocks`), verbatim from code. Transitions are declared by `declare_e2e_transitions!` in `crates/holon-integration-tests/src/pbt/transitions/`; invariant ids are declared by their bodies and correspondence families and assembled in `crates/holon-integration-tests/src/pbt/composed/catalog.rs`. |
| **Ruled by** | An ADR under [`docs/adr/`](../adr/), or an invariant number from [Model.md](Model.md). |
| **Mode axes** | Which of [Model.md](Model.md)'s four orthogonal axes the feature's behaviour varies on: **Storage** (Loro store on/off), **File adapter** (org/none), **Merge fidelity** (op-CRDT / base-3-way / LWW), **Transport** (iroh P2P on/off). `—` means the feature behaves the same everywhere. *Headless vs windowed* is a test-slice axis, not a product mode; it is called out in prose where it matters. |

The composed keystone is [`general_e2e_composed_pbt.rs`](../../crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs). Its alphabet is 73 transitions; the repo declares 75 invariant ids plus 17 correspondence-family ids. Open reds are registered in [KeystoneKnownReds.md](../Testing/KeystoneKnownReds.md) — a red listed there is a pass-with-note, anything else is a regression.

---

## Block editing

| Feature | What it is | Pinned by | Ruled by | Mode axes | Key entry point |
|---|---|---|---|---|---|
| Typing / text edit | Character keystrokes into a block's `MutableText` | `TypeChars`, `DeliverBlockContent`; `inv-editor-text/mirror`, `inv-block-content/block_raw` | Model.md inv 12 (every field write resolves a cell backing) | Merge fidelity (op-CRDT vs LWW text backing) | `crates/holon-core/src/cell_registry.rs` |
| Caret movement | Cursor keystrokes against `InputState` | `MoveCursor`, `ArrowNavigate`; `inv-editor-caret/mirror` | [0010](../adr/0010-focus-authority-in-memory-signal.md) — focus is in-memory, never persisted | — | `crates/holon-frontend/src/reactive_view_model.rs` |
| Split block | Enter at cursor splits a block in two | `SplitBlock`; `inv-birth-contract-satisfied` | [0030](../adr/0030-birth-atomicity-authority-and-mirror-contract.md) — births fire in the authority | Storage (Loro mints vs SQL mints) | `crates/holon-integration-tests/src/pbt/transitions/split_block.rs` |
| Join / backspace-merge | Backspace at start merges into the previous visible block | `JoinBlock`, `DeleteBackward` | [0030](../adr/0030-birth-atomicity-authority-and-mirror-contract.md) | Storage | `crates/holon-core/src/traits.rs` (`BlockOperations`) |
| Indent / outdent | Chords reparenting a block among its siblings | `Indent`, `Outdent`; `inv-no-parent-cycles`, `inv-no-orphan-blocks` | [0005](../adr/0005-children-as-ordered-list.md) — `sort_key` is an adapter detail | Storage | `crates/holon-core/src/traits.rs` |
| Reorder (chord + drag) | Alt+Up/Down and pointer drag move a block among siblings | `MoveUp`, `MoveDown`, `DragDropBlock`; `inv-birth-contract-satisfied` | Model.md inv 2, 3 — the consolidator mints every fractional index; intent carries `after_sibling`, never an order key | Storage (Loro-fi vs `gen_key_between`) | `crates/holon-loro/src/loro_block_operations.rs` |
| Structural ops as commit points | Pending editor text flushes through the merge path before a structural op runs | `Indent`, `Outdent`, `SplitBlock`; all preceded by the flush | Model.md inv 8 | Merge fidelity | [UI.md](UI.md) |
| Undo | Engine undo restores the pre-mutation block tree | `UndoLastMutation`; `inv-undo-redo-reference-heal` | [0031](../adr/0031-native-transition-catalog-and-macro-reification.md) | Storage | `crates/holon-core/src/undo.rs`, `crates/holon/src/core/operation_log.rs` |
| Redo | Redo re-executes the forward op and mints a **fresh** id | `Redo`; `inv-undo-redo-reference-heal` | [0031](../adr/0031-native-transition-catalog-and-macro-reification.md) | Storage | `crates/holon/src/core/operation_log.rs` |
| Slash commands | Slash-menu keystroke sequence resolving to a command | `TriggerSlashCommand`; `inv-viewmodel-editable-text-triggers` | [0024](../adr/0024-unified-action-execution.md) | — | `crates/holon-frontend/src/popup_menu.rs` |
| Templates | A canned inline template instantiates into real blocks | `InstantiateTemplate` | [0024](../adr/0024-unified-action-execution.md) | — | `crates/holon-frontend/src/popup_menu.rs` |

Open reds here: `editor-text-mirror`, `movecursor-unopened-editor`, `split-id-no-pairing`, `syn-real-mint`.

`syn-real-mint` is a block-loss detector, not a minting defect.

## Outline, rendering & widgets

| Feature | What it is | Pinned by | Ruled by | Mode axes | Key entry point |
|---|---|---|---|---|---|
| Render pipeline | query → rows → `EntityProfile.resolve` → ViewModel tree → platform view | `inv-viewmodel-root-matches-render-expr`, `inv-viewmodel-snapshot`, `inv-frontend-root-not-error` | [0018](../adr/0018-reactive-engine-viewmodel.md) | — | `crates/holon-api/src/entity_profile.rs` |
| Widget catalog | Per-widget shadow builders (`board`, `card`, `state_toggle`, …) | `inv-viewmodel-no-error-widgets`, `inv-displayed-text/viewmodel` | [0018](../adr/0018-reactive-engine-viewmodel.md) | — | `crates/holon-frontend/src/shadow_builders/` |
| `op_button` | A row-bound button dispatching a named op with its params | *(none — see Unpinned)* | [0024](../adr/0024-unified-action-execution.md) | — | `crates/holon-frontend/src/shadow_builders/op_button.rs` |
| `pie_menu` | Radial menu overlay; exempt from layout containment | *(only the layout-slice `assert_layout_ok` overflow allowance)* | [0026](../adr/0026-attention-environment-architecture.md) | — | `crates/holon-frontend/src/shadow_builders/pie_menu.rs` |
| Popups | One `PopupMenu` behind slash commands, wiki-links, mentions | `TriggerSlashCommand`; `inv-viewmodel-editable-text-triggers` | [0024](../adr/0024-unified-action-execution.md) | — | `crates/holon-frontend/src/popup_menu.rs` |
| Collapse / expand | Chevron and click gestures flip a block's collapsed gate | `ToggleCollapse`, `ExpandToggle`; `inv-embedded-page-collapsed-lazy` | Model.md — `Mutable<T>` is per-render-slot, never registry-keyed | — | `crates/holon-frontend/src/reactive_view_model.rs` |
| Drawers | Property/metadata drawer open-close | `ToggleDrawer`; `inv-drawer-open-matches-ref` | — | — | `crates/holon-frontend/src/shadow_builders/` |
| Inline text styling | Bold/italic/underline/strike/code/link marks at paint level | `inv-paint-text-styling`, `inv-mark-bounds-within-content` | — | — | `crates/holon-org-format/src/models.rs` |
| Virtual slots / lazy rows | Off-screen rows are slots, not materialized widgets | `inv-viewmodel-tree-virtual-slots`, `inv-journal-feed-viewport-lazy` | [0018](../adr/0018-reactive-engine-viewmodel.md) | — | `crates/holon-frontend/src/reactive_view_model.rs` |
| Scroll & sticky headers | Two-mode wheel motion, occlusion routing, sticky accordion | `WheelScroll`; `inv-wheel-two-mode-motion-law`, `inv-wheel-occlusion-routing`, `inv-sticky-accordion-spec` | [0026](../adr/0026-attention-environment-architecture.md) | — | `frontends/gpui/src/` (windowed slice only) |
| Advice weave | Top-K suppression-filtered advice rows woven at each anchor | `inv-advice-rows-woven` | [0021](../adr/0021-advice-suppression-storage-and-readonly-v1.md), [0022](../adr/0022-runtime-definable-advice-rules.md), [0023](../adr/0023-two-stage-relevance-app-layer-reranker.md) | — | `crates/holon/src/api/ui_watcher.rs` |
| Display placement | A row shown somewhere it does not canonically live | `inv-display-placement-canonical-inert`; env-gated on `HOLON_PBT_DISPLAY_PLACED` | [0015](../adr/0015-computed-placement-and-curated-state-primitives.md) *(Proposed, not implemented)* | — | [ADR 0015](../adr/0015-computed-placement-and-curated-state-primitives.md) |

Open reds here: `lib-displayed-text-nested-content-skipped`.

## Navigation, focus & panes

| Feature | What it is | Pinned by | Ruled by | Mode axes | Key entry point |
|---|---|---|---|---|---|
| Click-to-focus | Pointer click resolves a click intent and moves focus | `ClickBlock`, `FocusEditableText`; `inv-focus-matches-ref`, `inv-navigation-focus` | [0010](../adr/0010-focus-authority-in-memory-signal.md) | — | `crates/holon-frontend/src/operations.rs` |
| Page navigation | Navigating a region to a new focus root | `NavigateFocus`; `inv-focus-roots`, `inv-main-panel-rows-match-focus` | [0016](../adr/0016-occurrence-keyed-focus-authority.md) *(Proposed)* | — | `crates/holon-frontend/src/operations.rs` |
| Back / forward / home | Per-region navigation history | `NavigateBack`, `NavigateForward`, `NavigateHome` | [0010](../adr/0010-focus-authority-in-memory-signal.md) | — | `crates/holon-frontend/src/operations.rs` |
| Open in tab | Cmd+click a sidebar page into a new tab | `OpenTabViaModifierClick` | [0026](../adr/0026-attention-environment-architecture.md) | — | `frontends/gpui/src/views/render_entity_view.rs` |
| Pin to sidebar | Shift+click a bullet pins it to the right sidebar | `PinBlock`, `UnpinBlock`; *no invariant observes pinned state* | [0026](../adr/0026-attention-environment-architecture.md) | — | `assets/default/types/block_profile.yaml` (`shift_action` wiring) |
| Sidebar | Page list, watch-seeded, click routes to the main panel | `SwitchView`; `inv-sidebar-page-tag-preserved` | — | — | `crates/holon-frontend/src/shadow_builders/` |
| View modes | Switching a region's view mode (outline / board / …) | `SwitchViewMode`, `SwitchView`; the derived pair `inv_pair_view_selection_current_view()` | [0018](../adr/0018-reactive-engine-viewmodel.md) | — | `crates/holon-frontend/src/reactive_view_model.rs` |

Open reds here: `lib-nav-history-shift-action-region`, `lib-pin-block-right-sidebar-probe`, `lib-unpin-block-probe`, `lib-focus-roots-mismatch-right-sidebar`, `lib-right-sidebar-renders-pins`, `lib-sut-only-pin-not-caught`, `pinblock-unrendered-target`.

The pin / right-sidebar cluster is the largest open family in the registry: one region-literal typo (`right` vs `right_sidebar`) accounts for all of it.

## Queries & projections

| Feature | What it is | Pinned by | Ruled by | Mode axes | Key entry point |
|---|---|---|---|---|---|
| Query languages | PRQL / GQL / SQL, all compiled to plain SQL | `inv-viewmodel-decompiled-rows-match-query` | [Architecture.md §Multi-Language Query Support](../Architecture.md) | — | `crates/holon-api/src/types.rs` (`QueryLanguage`) |
| Degraded no-query mode | With no query engine, a block shows its source | `inv-viewmodel-shows-source-when-no-query`, `inv-source-language-iff-source` | — | — | `crates/holon-frontend/src/render_interpreter.rs` |
| Filters | `holon_filter` source blocks resolving to a typed `FilterSpec` | `inv-filter-spec-resolves` | — | — | `crates/holon-turso/src/` |
| Matviews / IVM | Incrementally maintained materialized views over blocks | `inv-matview-consistent-with-recompute`, `inv-typed-matview-matches-ref`, `inv-matview-consistent-with-ref/root_layout` | Model.md layer 3 — exactly one writer, verbatim and total | Storage | `crates/holon-turso/src/matview_manager.rs` |
| CDC → LiveData | Convergent-state stream feeding cells and the ViewModel | `inv-live-children-match-ref`, `inv-live-tree-matches-fresh`, `inv-frontend-engine` | [0018](../adr/0018-reactive-engine-viewmodel.md); Model.md layer 4 | Storage | `crates/holon-api/src/live_data.rs` |
| Block history | An op-grounded history table over block writes | `inv-history-no-phantom-rows/block_history`, `inv-history-records-all-creates/block_history` | [0025](../adr/0025-op-grounded-projections.md) | Storage | [Schema.md](Schema.md) |
| Schema & typed entities | Runtime-declared types projected into their own matviews | `DeclareTypedSchema`, `CreateTypedEntity`, `RegisterEntityScheme`, `ConcurrentSchemaInit`; `inv-typed-matview-matches-ref` | [0029](../adr/0029-entity-identity-single-minting-authority.md) | — | `crates/holon-turso/src/dynamic_schema_module.rs` |

Open reds here: `history-ingest-create-unrecorded`, `history-join-phantom-row`.

## Rules & automation

| Feature | What it is | Pinned by | Ruled by | Mode axes | Key entry point |
|---|---|---|---|---|---|
| PN action language | Petri-net transitions are the sole action surface; Rhai-compiled guards | *(no keystone transition drives net firing directly)* | [0024](../adr/0024-unified-action-execution.md), [0017](../adr/0017-petri-net-task-ranking-engine.md) | — | `crates/holon-engine/src/` |
| `holon_rule` blocks | Single-block YAML rules with `when:`/`emit:` arcs | `AdvanceDay`; `inv-journal-one-per-day` | [0024 §7.2](../adr/0024-unified-action-execution.md) | — | `crates/holon/src/api/holon_rule_watcher.rs` |
| Query watches | Registering a query watch and its CDC subscription | `SetupWatch`, `RemoveWatch`; `inv-active-watches-match-ref`, `inv-watch-rows-match-ref` | [0018](../adr/0018-reactive-engine-viewmodel.md) | — | `crates/holon/src/api/action_watcher.rs` |
| Task ranking | Task blocks materialized into a Petri net for WSJF ranking | *(no keystone invariant)* | [0017](../adr/0017-petri-net-task-ranking-engine.md) | — | `crates/holon-petri/src/lib.rs` |
| Task state cycle | `state_toggle` click cycles a task through its states | `ToggleState`; `inv-task-state-matches-ref`, `inv-task-state-storage-coherence`, `inv-viewmodel-state-toggle-correct` | [0024](../adr/0024-unified-action-execution.md) | Storage (the coherence twin needs the Loro task-state cap) | `crates/holon-frontend/src/shadow_builders/state_toggle.rs` |

Open reds here: `task-state-storage-coherence`, `state-toggle-row-absent`.

## Storage & sync

| Feature | What it is | Pinned by | Ruled by | Mode axes | Key entry point |
|---|---|---|---|---|---|
| Loro CRDT store | Durable replica with op history and frontiers | `SimulateRestart`, `CreateStaleLoro`; `inv-blocks-match-ref/loro`, `inv-loro-children-match-ref`, `inv-loro-no-errors` | [0003](../adr/0003-all-in-lorotree-architecture.md) + [amendment](../adr/0003-amendment-multi-tree.md) | Storage; Merge fidelity | `crates/holon-loro/src/loro_sync_controller.rs` |
| Turso projection | Single-writer, total, verbatim projection of the consolidated tree | `inv-blocks-match-ref/block_raw`, `inv-blocks-match-ref/matview`, `inv-block-parent/block_raw` | Model.md inv 4, 5 | Storage | `crates/holon-turso/src/turso.rs` |
| SqlOnly mode | Loro store off; Turso-LWW is the consolidator | *(whole keystone runs in both arms; SqlOnly deselects the Loro-cap invariants — disclosed, not faked)* | Model.md §Four orthogonal mode axes | Storage; Merge fidelity | `crates/holon/src/core/sql_operation_provider.rs` |
| Consolidator epoch guard | A mode flip without a base re-seed is refused at startup | `EpochFlipRejected` | Model.md inv 10 | Storage | `crates/holon-app/src/consolidator_epoch.rs` |
| Org file round-trip | Disk org → parse → blocks → render → byte-identical disk org | `WriteOrgFile`, `BulkExternalAdd`, `DenseProjectionEdit`; `inv-org-render-fixed-point`, `inv-blocks-match-ref/org` | [0014](../adr/0014-doc-scheme-retirement.md); [ORG_SYNTAX.md](../Reference/ORG_SYNTAX.md) | File adapter | `crates/holon-org-format/src/models.rs` |
| 3-way diff ingest | Inbound intent is `diff(base, current)` per replica | `StaleExternalRewrite`, `ExternalWriteWhileFocused`, `ExternalWriteSameBlockFocused` | Model.md inv 1, 6 | File adapter; Merge fidelity | `crates/holon-orgmode/src/` |
| Page identity / name chains | A page's file path is its chain of **page** ancestors | `RenamePage`, `BlockToPage`, `CreatePageAtFreedPath`; `inv-no-page-under-non-page`, `inv-every-page-has-its-own-file`, `inv-companion-has-no-child-page-headings` | [Model.md §Page identity](Model.md) (Martin ruled Option A, keep-the-refusal) | File adapter | `crates/holon-filesystem/src/sync_ports.rs` |
| Documents & directories | Creating, renaming, deleting docs and dirs on disk | `CreateDocument`, `RenameDocument`, `DeleteDocument`, `CreateDirectory`; `inv-no-write-outside-vault-root` | [0011](../adr/0011-filesystem-port-trait.md) | File adapter | `crates/holon-filesystem/src/` |
| Tombstones | A tombstone outlives every registered replica's base | *(no keystone invariant)* | Model.md inv 9 | Storage | [Replication.md](Replication.md) |
| P2P sync | Iroh transport carrying Loro deltas between devices | `AddPeer`, `PeerEdit`, `PeerCharEdit`, `SyncWithPeer`, `MergeFromPeer`, `SyncNow` | [0001](../adr/0001-hybrid-sync-architecture.md); Model.md inv 11 | Transport | `crates/holon-loro/src/iroh_sync_adapter.rs` |
| Git / jj vault init | Initializing version control over the vault directory | `GitInit`, `JjGitInit` | — | File adapter | `crates/holon-filesystem/src/` |

Open reds here: `org-blocks-ref-diverge`, `page-without-own-file`, `loro-frontier-height`.

`org-blocks-ref-diverge` is half fixed — cause A (undo over-reverting file-ingested content) is locked by a keystone regression; cause B, the `::img::0` sub-block id remap, is open.

## Homes & re-homing

| Feature | What it is | Pinned by | Ruled by | Mode axes | Key entry point |
|---|---|---|---|---|---|
| Home / capability profiles | Which durable format an entity lives in, derived (never stored) | `inv-home-profile-matches-derived` | [0012](../adr/0012-reference-model-capability-contract.md), [0019](../adr/0019-capmap-dependency-injection.md) | File adapter | `crates/holon-capability/src/home.rs` |
| `rehome_entity` | Moving a file-homed leaf into Holon's own storage | `RehomeEntity`; `inv-home-profile-matches-derived` | [0019](../adr/0019-capmap-dependency-injection.md) | File adapter | `crates/holon-app/src/rehome_entity.rs` |
| Profile assets | The declarative profiles shipped with the app | *(consumed by every profile-resolving invariant)* | [RenderPipeline.md](RenderPipeline.md) | — | `assets/default/types/`, `assets/default/capability/holon-native.yaml` |

## Integrations

| Feature | What it is | Pinned by | Ruled by | Mode axes | Key entry point |
|---|---|---|---|---|---|
| MCP client sidecars | YAML-declared external connectors bridged to `OperationProvider` | `EmitMcpData`; CDC re-eval / duplicate detection only | [0001](../adr/0001-hybrid-sync-architecture.md), [0006](../adr/0006-actor-terminology-and-mcp-dual-role.md) | — | `crates/holon-mcp-client/src/mcp_sidecar.rs` |
| Todoist | The reference connector: MCP over HTTP, static token, oauth false, sync policy, undo | *(none — see Unpinned)* | [Integrations.md](Integrations.md) | — | `assets/integrations/todoist.yaml` |
| Other sidecars | gcal, gmail, claude-history, jsonplaceholder | *(none)* | [Integrations.md](Integrations.md) | — | `assets/integrations/` |
| Write authorization | What an integration is permitted to write back | *(none in the keystone)* | [0028](../adr/0028-sharing-policy-overlay.md) (adjacent) | — | `crates/holon-mcp-client/src/write_authorization.rs` |
| LogSeq DB import | Reads a LogSeq DB graph (Transit-JSON in SQLite `kvs`) into blocks | *(standalone test `logseq_db_import_store.rs`, not the keystone)* | — | File adapter | `crates/holon-logseq-db/src/ingest.rs` |
| LogSeq write-back | Pushes title and `:block/tags` changes back as LogSeq tail transactions | *(standalone tests only)* | — | File adapter | `crates/holon-logseq-db/src/kvs_writer.rs` |

## Journals, tags & properties

| Feature | What it is | Pinned by | Ruled by | Mode axes | Key entry point |
|---|---|---|---|---|---|
| Daily journal pages | The `daily_journal` rule emits exactly one page per calendar day | `AdvanceDay`; `inv-journal-one-per-day` | [0024 §6](../adr/0024-unified-action-execution.md) | File adapter | `crates/holon/src/api/holon_rule_watcher.rs` |
| Journals feed | Day pages expand exactly while inside the rendered window | `inv-journal-feed-viewport-lazy` | [0018](../adr/0018-reactive-engine-viewmodel.md) | — | *(windowed slice only)* |
| Tags | `Block.tags`, an edge field backed by the `block_tags` junction | `SetEdgeField`; `inv-block-tags-references-exist` | [0005](../adr/0005-children-as-ordered-list.md) | — | `crates/holon-api/src/block.rs` |
| The `Page` tag | The literal tag that makes a block a page | `inv-sidebar-page-tag-preserved`, `inv-no-page-under-non-page` | [Model.md §Page identity](Model.md) | File adapter | `crates/holon-api/src/block.rs` |
| Org properties | Drawer key/values round-tripping through the store | `inv-blocks-match-ref/org` | [CompassConventions.md](../Reference/CompassConventions.md) | File adapter | `crates/holon-org-format/src/models.rs` |

Known hazards, not bugs to re-discover: `_`-prefixed property **keys** are erased on write-back, and an empty property value drops its key entirely (`crates/holon-org-format/src/models.rs`). Authored drawer order **survives** both production write legs via the `_drawer_order` carrier.

## Sharing

| Feature | What it is | Pinned by | Ruled by | Mode axes | Key entry point |
|---|---|---|---|---|---|
| Policy overlay | Sharing is a policy overlay over aligned containers | `ShareContainer`; `inv-audience-never-over-approximates` | [0028](../adr/0028-sharing-policy-overlay.md) | Transport | `crates/holon-sharing/src/policy.rs` |
| Two-instance convergence | The receiver holds exactly what the owner's policy sanctions | `SyncNow`; `inv-two-instance-convergence`; *two-instance slice only* | [0028](../adr/0028-sharing-policy-overlay.md) | Transport | `crates/holon-integration-tests/tests/two_instance_composed_pbt.rs` |
| Crossing log & arbitration | Owner-scoped totally-ordered log with deterministic conflict resolution | `tests/inc4_pbt.rs` (crate-local, not the keystone) | [0028](../adr/0028-sharing-policy-overlay.md) | — | `crates/holon-sharing/src/log.rs` |
| Alias ledger | Owner-private old-id → new-id succession chains | `tests/inc56_pbt.rs` (crate-local) | [0028](../adr/0028-sharing-policy-overlay.md) | — | `crates/holon-sharing/src/alias_ledger.rs` |
| Mount boundary | A shared subtree's org file is a one-way projection sink | *(oracles are taught to MAP the Loro↔SQL shape difference; no dedicated invariant)* | Model.md inv 11 (disclosed exception) | Transport; File adapter | [Model.md](Model.md) |
| Encrypted relay | Server-blind relay transport | *(design only)* | [EncryptedRelaySync.md](EncryptedRelaySync.md) — design discussion captured 2026-07-20 | Transport | *(not implemented)* |

Ratified design; increments 4–6 exist in code, the end-to-end feature does not ship yet.

## Latency & budgets

| Feature | What it is | Pinned by | Ruled by | Mode axes | Key entry point |
|---|---|---|---|---|---|
| Interaction latency SLO | p95 interaction → projection-visible under 200ms | `inv-settle-budget` | Project rule (CLAUDE.md): a breach is a **bug**, triaged into the bug funnel | Storage | `crates/holon-integration-tests/src/pbt/invariants/bodies/settle_budget.rs` |
| SQL / wall / RSS budget | Per-transition resource ceiling | `inv-sql-budget`; `otel-testing` feature | — | Storage | `crates/holon-integration-tests/src/pbt/transition_budgets.rs` |
| Complexity-class trend | An O(1)-claiming transition must not grow with sequence position | `inv-complexity-class-trend`; *observe-only until `HOLON_TREND_BUDGET=1`* | — | — | `crates/holon-integration-tests/src/pbt/complexity_trend.rs` |
| Steady-state reseed | No interactive transition triggers an O(N) full reseed | `inv-no-steady-reseed-leak`; *observe-only until `HOLON_PBT_RESEED_ORACLE=enforce`*; soak rung `soak_reseed_reproduction` | — | Storage | `crates/holon-integration-tests/src/pbt/composed/reseed_observer.rs` |
| Navigation at vault scale | Cold focus-descendant matview cost at ~2000 blocks | soak rung `soak_nav_latency` | — | Storage | `crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs` |
| Swallowed errors | No ERROR-level event or panic escaped a transition | `inv-no-errors`, `inv-no-observed-errors`, `inv-no-declared-column-absent`; *the last observe-only* | CLAUDE.md — never swallow errors | — | `crates/holon-integration-tests/src/pbt/invariants/bodies/no_errors.rs` |

The keystone's own scale is small (a 3-block focus doc), so the vault-scale behaviours above are only reachable through the soak rungs. That is the single biggest structural blind spot in the pin coverage.

## Frontends

| Feature | What it is | Pinned by | Ruled by | Mode axes | Key entry point |
|---|---|---|---|---|---|
| GPUI desktop (macOS) | The primary frontend | Windowed slice: `inv-frontend-bounds-rendered`, `inv-window-focus-matches-engine-focus`, `inv-live-block-shell-present`, `inv-inline-row-mount-present`, `inv-paint-text-styling`, `inv-displayed-text/widget` | [0026](../adr/0026-attention-environment-architecture.md) | — | `frontends/gpui/src/` |
| iOS | GPUI core in a native shell | *(none in the keystone)* | — | — | `frontends/gpui/ios/` |
| Android | GPUI core in a native shell with a Java text-input view | *(none in the keystone)* | — | — | `frontends/gpui/android/` |
| Dioxus web | Backend + engine in a Web Worker, DOM on the main thread | `tests/web_arm_keystone.rs` | — | — | `frontends/dioxus-web/` |
| MCP server frontend | Every frontend launches one; also the live-PBT driver | `general_e2e_composed_pbt_live_mcp` *(gated on `HOLON_PBT_LIVE_MCP`)* | [0006](../adr/0006-actor-terminology-and-mcp-dual-role.md) | — | `frontends/mcp/src/tools.rs` |
| TUI | Terminal frontend | *(none)* | — | — | `frontends/tui/` |
| Shared ViewModel layer | The platform-agnostic layer all frontends consume | The whole headless keystone | [0018](../adr/0018-reactive-engine-viewmodel.md) | — | `crates/holon-frontend/src/reactive_view_model.rs` |

Open reds here: `tui-focus-propagate`.

---

## Unpinned features

Rows above whose coverage a human judged incomplete. No automated test drives these — a change here is only as safe as the reasoning behind it.

- **`op_button`** — the widget that dispatches a named op with its params. No keystone transition clicks one and no invariant observes the dispatch. Given [ADR 0024](../adr/0024-unified-action-execution.md) makes ops the sole action language, this is the most load-bearing unpinned row in the map.
- **`pie_menu`** — only the layout slice touches it, and only as an *exemption* from the containment assertion. Nothing checks that it renders correctly.
- **Display placement** — [ADR 0015](../adr/0015-computed-placement-and-curated-state-primitives.md) is Proposed and unimplemented; the invariant exists but is inert unless `HOLON_PBT_DISPLAY_PLACED` is set.
- **Pin to sidebar** — `PinBlock`/`UnpinBlock` drive the op, but no invariant reads pinned state back. The open `lib-*pin*` known-reds live in exactly this hole.
- **PN action language** — the Petri-net engine has no keystone transition or invariant. Only `holon_rule` blocks (the YAML rule surface) are pinned, via the journal rule.
- **Task ranking** — WSJF ranking over the task net is exercised by nothing in the keystone.
- **Tombstones** — stated as an invariant of the system (Model.md invariant 9), checked by no invariant of the test suite. A premature GC would resurrect deleted blocks on a stale replica's next diff.
- **Todoist** — `EmitMcpData` covers CDC re-emission only. Nothing exercises the reference connector's sync, auth, write-back, or undo.
- **Other sidecars** — the same hole as Todoist, once per connector.
- **Write authorization** — no keystone coverage of what a connector may write back.
- **LogSeq DB import** — covered by standalone crate tests, not by the keystone, so no cross-subsystem interaction is exercised.
- **LogSeq write-back** — the write leg has the same standalone-only coverage as the import leg.
- **Encrypted relay** — design only, nothing built.
- **iOS** — the native shell has no automated coverage; the shared GPUI core is covered only through the macOS windowed slice.
- **Android** — same as iOS, plus the Java text-input view no test drives.

## Unclaimed by any row

Declared in the sources, claimed by no row above. A new transition or invariant lands here automatically, so a coverage gap never depends on someone remembering to write it down.

### Transitions

- `ApplyMutation` — org vs Loro ingress the shrinker can localize
- `CreateBlockUnderFocus` — creation-slot gesture mint under focus root
- `Nothing` — no-op interleaving for schedule diversity
- `PressKey` — raw key chord -> bubble_input resolution
- `ReceiverCreateBlock` — a peer-authored block under an owner-authored parent, and its arrival on the owner after a reverse round.
- `StartApp` — application startup + seeded sidebar watch

### Invariant ids

- `inv-advice-matview-matches-ref/matview`
- `inv-block-content/sql`
- `inv-block-ids-match-ref` — SQL projection block-id set vs ref non-seed block ids (coarse set equality)
- `inv-boundary-respected` — a cross-instance sharpening of `inv-audience-never-over-approximates`: that one asserts alignment inside ONE model, this one asserts it across a real second SUT.
- `inv-decompiled-rows-rendered`
- `inv-editable-text-has-draggable` — every editable_text/rendered_text in a block-profile subtree paired with a same-id draggable
- `inv-editor-caret-matches-ref`
- `inv-frontend-no-error-widgets` — no Error widget in the laid-out BoundsRegistry (authoritative) or, absent geometry, the ViewModel tree
- `inv-net-totality` — an operation the system can fire that the derived net does not describe, so every net-based analysis (conflicts, cycles, the marking oracle) silently reports on a partial world
- `inv-shows-source-when-no-query`
- `inv-state-toggle-toy`
- `inv-two-writer-peer-writes-land` — generic ComposedSut StateMachineTest: per-tick reconcile + catalog check + non-vacuity floor
- `inv-value-fn-provider-arg-variance-13` — the ReactiveEngine / interpret_pure / ProviderCache coupling drops rows, churns Arc identity, or flickers
- `inv-value-fn-provider-identity` — a transient wrong StateToggle in an intermediate emission that a later structural re-render masks
- `inv-viewmodel-entity-ids-subset-of-data` — a rendered entity id that is neither a root query-data row nor a ref-known block
- `inv-viewmodel-task-rows-have-state-toggle` — a rendered collection row backed by a task block carries no `state_toggle` in its own row scope (the flat-live-query page_title misfire: bugfunnel 2026-08-25-flat-query-task-rows-render-as-page-title-blobs)

### Known reds

- `bulk-add-sibling-order-under-journals`
- `deletebackward-sql-reads-budget`
- `drawer-open-matches-ref`
- `lib-type-chars-home-profile-derived`
- `loro-backend-change-count`
- `opentab-sql-reads-budget`
- `pinblock-lazy-day-page-shell`
- `proptest-sm-shrink-seen-transitions`
- `turso-block-query-source-round-trip`
- `typechars-sql-reads-budget`
- `vault-scale-main-panel-delivery`

## Sources

Every derived part of this page, and where it comes from.

| Content | Source of truth |
|---|---|
| The transition list | `declare_e2e_transitions!` in `crates/holon-integration-tests/src/pbt/transitions/mod.rs` — an arch test already asserts one file per variant. |
| One-clause descriptions of unclaimed names | the `@pbt covers <slug> — <clause>` annotation of the module that declares the transition or the invariant id. |
| The invariant id universe | every `InvariantId("inv-…")` body, `capability_pair!` id override, and correspondence-family `id: "inv-…"` under `crates/`. |
| Open reds per area | [KeystoneKnownReds.md](../Testing/KeystoneKnownReds.md) — a key is listed only while its `Status` is `known-red`. |
| Path and link freshness | the working tree; a cell naming a file that does not exist fails generation. |

What stays hand-written in [`featuremap.yaml`](featuremap.yaml): the grouping of transitions and invariants into features Martin would name, the one-clause descriptions, and the "Ruled by" column linking each to its decision.
