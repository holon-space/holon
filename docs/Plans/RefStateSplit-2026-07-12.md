# Plan: Splitting the keystone reference model into sub-ref-states

**Date:** 2026-07-12
**Status:** PROPOSED (senior-review pending)
**Target:** `crates/holon-integration-tests/src/pbt/reference_state.rs` (2,398 lines) and
`crates/holon-integration-tests/src/pbt/reference_capabilities.rs` (2,181 lines)
**Prior art (binding):** ADR 0012, `docs/Testing/PbtCompositionDesign.md` §5.1/§5.4/§5.5,
`docs/Testing/PbtSlicing.md`, ADR 0004 Phases 2–6 (fragment extraction, landed),
Phase 1a de-concretization (47/54 transitions on `<R: Ref*>`, commit `e831f0bd`).

---

## 0. Senior-review of the implied plan ("split ReferenceState into sub-ref-states")

There is **no prior standalone split-plan document** — but there IS a ratified design
that constrains any split, and a large fraction of the naive split is **already landed**.
Any plan that ignores either fact produces rework:

1. **"Ref core does not compose" is ratified.** `PbtCompositionDesign.md` §5.1: the
   reference model is *one omnipotent, coupled core*; sub-Refs are **projections
   (capability traits), never a union of parts**. §5.4 sharpens it: shared block data
   must be **single-homed**; two copies + reconcile-on-read is the named anti-pattern.
   §5.5 then gives the sanctioned decomposition: **one coupled core + an open registry
   of per-module *private-state* extensions**, and names the exact backlog item this
   plan implements: *"(b) replacing `ReferenceState`'s hardcoded private fields with
   the open extension registry (the core keeps its cross-cutting fields; only
   subsystem-private state moves into the registry)."*
   → The split axis is therefore **private vs. cross-cutting state**, not "one
   sub-state per Ref\* trait". A trait-per-struct split of the *core* (blocks / editor /
   focus) would violate §5.5's "core stays one provider owning all the cross-cutting
   caps (block+editor+focus from one object)".

2. **The state-fragment split is ~70% landed** (ADR 0004 Phases 2–6). `ReferenceState`
   already composes:
   - `domain: ReferenceDomainState` (`reference_domain_state.rs`) — block tree, layout
     classification, profiles, render exprs
   - `ui: UIActorState` (`ui_actor_state.rs`, split per-tab/per-user in Phase 6)
   - `action: ActionActorState`, `mcp: MCPServerActorState`, `files: FileAdapterState`
   What is **not** landed: (a) the supporting *types* (`BlockState`, `LayoutBlockInfo`,
   `ActiveEditor`, `CursorPosition`, `NavigationHistory`, `OpenPinEntry`, `PeerRefState`,
   `ClockState`) and ~100 methods still live in the monolithic `reference_state.rs`;
   (b) Loro-peer state (`peers`, `shadow_mesh`, `clock_feed`) is still loose top-level
   fields, not a fragment, and not co-located with the Loro subsystem crate;
   (c) `reference_capabilities.rs` is one 2,181-line forwarding file;
   (d) misc harness residue (`runtime`, `wiring`, `cap_set`, `real_editor`,
   `interpreter`, pre-startup file/git/jj flags) is interleaved with model state.

3. **The capability contract is the stable seam.** 36 `Ref*` traits in
   `crates/holon-pbt-core/src/capabilities.rs`; 47/54 transitions and the invariant
   bodies already program against `<R: Ref*>`, never the concrete type
   (`holon-loro-testing/src/transitions/*.rs` already do this from a *separate crate*).
   So sub-state extraction is invisible to transitions/invariants as long as the trait
   impls keep their semantics — this is what makes the migration mechanical at all.

**Verdict on the naive framing:** do NOT split `ReferenceState` into independently
ownable sub-ref-states for blocks/editor/focus. DO finish the fragment extraction,
split the forwarding monolith along trait clusters, extract the **Loro-private
extension** into `holon-loro-testing` (the co-location north star's next concrete
step), and only then decide on the open registry (§5.5 backlog (b)) — which is an
architecture fork needing a Martin ruling, not something to bundle into this refactor.

---

## 1. Target decomposition

```
ReferenceState (composition root; stays the single CapProvider for cross-cutting caps)
├── domain:  ReferenceDomainState        [exists]  + absorbs BlockState, LayoutBlockInfo defs
│     serves: RefBlockTree(Mut), RefDocuments(Mut), RefBackend, RefTaskState,
│             RefLayout, RefRenderExpr, RefSqlCardinality, RefAdvice, RefApplyMutationMut
├── ui:      UIActorState (tab/user)     [exists]  + absorbs ActiveEditor, CursorPosition,
│     NavigationHistory, OpenPinEntry defs
│     serves: RefFocus(Mut), RefEditorMirror(Mut), RefNavHistory(Mut), RefPins(Mut),
│             RefToggle(Mut), RefViewSelection(Mut), RefGlobalFocus, RefFocusRoots,
│             RefArrowNav (read side)
├── action:  ActionActorState            [exists]  + absorbs ClockState def
│     serves: RefLifecycle (part), RefClock(Mut)
├── mcp:     MCPServerActorState         [exists]
│     serves: RefWatch, RefWatchesMut
├── files:   FileAdapterState            [exists]  + absorbs pre_startup_directories,
│     pre_startup_file_count, git_initialized, jj_initialized
│     serves: RefBoot(Mut) (file/boot part)
├── loro:    LoroRefExt   ★ NEW, defined in crates/holon-loro-testing ★
│     = peers: Vec<PeerRefState> + shadow_mesh: Option<ShadowMesh> + clock_feed
│     serves: RefPeers, RefPeersMut (thin orphan-rule impls stay in integration-tests,
│             delegating to inherent methods on LoroRefExt + core write interface)
└── harness: HarnessEnv   ★ NEW module-local struct ★
      = runtime, wiring, cap_set, real_editor, interpreter
      serves: RefWiring; NOT model state — explicitly separated so `Clone` semantics
      (shared clock_feed cell, Arc'd runtime/interpreter) are visible in one place
```

**Stays on the composition root (cross-fragment by necessity, per ADR 0012 §5):**
`blur_active_editor` / `commit_active_editor_if_changed` (editor→blocks commit
contract), `clear_focus_if_deleted` / `reset_cursor_if_focused` (blocks→focus),
`split_block` / `join_block` / `move_block` / `outdent_block` (blocks + editor + focus),
`recanon_and_rebuild` / `rebuild_profile_tracking` (blocks→profiles→render exprs),
`push_undo_snapshot` / `pop_undo_to_redo` (action stack clones domain `BlockState`),
`shadow_catch_up_primary` (core blocks → loro ext), `with_resolved_doc_uris` /
`remapped_doc_uris` / `Resolved<T>`, the whole `impl BuilderServices for ReferenceState`
(spans domain + ui + interpreter + runtime), `expected_focus_root_ids` (ui pins → SQL
matview prediction), layout/render prediction helpers (`active_main_query`,
`main_rendered_block_ids`, `renders_block_interactively`, …: domain + ui + interpreter).

That residue is the *point* of the coupled core — after the split, `reference_state.rs`
should contain approximately: the struct, constructor/builders, the cross-fragment
contract methods above, and nothing else (~600–800 lines, from 2,398).

**File layout after the split** (all under `crates/holon-integration-tests/src/pbt/`
except the Loro ext):

```
refstate/                       (new directory module; reference_state.rs shrinks into it)
├── mod.rs                      ReferenceState struct + cross-fragment methods + Resolved
├── builder_services.rs         impl BuilderServices + view_model_has_widget + block_to_data_row
├── layout_predict.rs           active_main_query / rendered-set / interactivity helpers
domain: reference_domain_state.rs  + block_state.rs (BlockState, LayoutBlockInfo + their impls)
ui:     ui_actor_state.rs          + ui_types.rs (ActiveEditor, CursorPosition,
                                     NavigationHistory, OpenPinEntry + impls)
action: action_actor_state.rs      + clock_state.rs (ClockState)
ref_caps/                       (split of reference_capabilities.rs, one file per cluster)
├── mod.rs                      reference_state_ref_caps() + CapProvider impl + helpers
├── blocks.rs  editor.rs  focus.rs  nav.rs  docs.rs  boot.rs  layout.rs
├── watches.rs  toggle.rs  clock.rs  advice.rs  misc.rs
└── peers.rs                    thin RefPeers(Mut) impls delegating to LoroRefExt
crates/holon-loro-testing/src/ref_ext.rs   PeerRefState + LoroRefExt inherent logic
crates/holon-loro-testing/src/shadow_mesh.rs  (moved from integration-tests)
```

---

## 2. Ownership: which transitions touch which fragments

From the Phase 1a sweep (47/54 generic) plus the trait clusters:

| Fragment | Transitions writing it (via `Ref*Mut` bounds) |
|---|---|
| domain | SplitBlock, JoinBlock, CreateBlock*, Indent/Outdent, SwapSequence, content edits, ApplyMutation, task-state toggles, layout mutations |
| ui | ClickBlock, FocusEditableText, ArrowNavigate, NavigateFocus/Back/Forward, ToggleDrawer/Collapse/Expand, SwitchViewMode, pin ops |
| domain+ui jointly | the seven T0 structural/editing transitions (commit-then-mutate contract — they go through the composition-root methods, never fragment-direct) |
| action | StartApp/Restart (lifecycle), AdvanceDay (clock), Undo/Redo |
| mcp | watch add/remove |
| files | pre-startup file ops, org-file boot seeding |
| loro ext | AddPeer, PeerEdit, PeerCharEdit, SyncWithPeer, MergeFromPeer (already in `holon-loro-testing/src/transitions/`, already generic over `R: RefPeersMut + RefBlockTreeMut …`) |

**The residual concrete-`ReferenceState` transitions** (grep 2026-07-12, this tree):
`toggle_drawer.rs`, `toggle_collapse.rs`, `switch_view_mode.rs`,
`deliver_block_content.rs` — orphan-anchors on shared `holon_layout_testing` structs
(impls on `LayoutRef<'_, R>`); `start_app.rs` — boot-seed oracle helpers (intentional);
`navigate_back.rs` — test scaffolding; `mod.rs` — the `declare_e2e_transitions!`
assembler (intentionally concrete); `apply_mutation.rs` — **already de-concreted**
(body generic; remaining mentions are comments only; commits `5ce17cc5…` etc. are in
this history). None of these blocks the split: they hold `&mut ReferenceState` and the
composition root keeps inherent mirrors/delegators, so they compile unchanged.

> **Staleness guard (each implementer, at increment start):** this residual audit was
> taken on the plan's base rev. Before touching transitions, re-run
> `rg -l "ReferenceState" crates/holon-integration-tests/src/pbt/transitions/` and diff
> against the list above — the Phase 1a sweep and adjacent streams keep de-concreting
> residuals, so a file may have dropped off (cheap; catches drift).

---

## 3. Migration sequence (small increments, keystone green after each)

Verification command for every increment (the DONE gate):

```
cargo check -p holon-integration-tests --features pbt --all-targets > /tmp/refsplit-check.log
cargo nextest run -p holon-integration-tests -E 'test(general_e2e_composed_pbt)' \
  > /tmp/refsplit-keystone.log   # bounded default case count; plus the persisted regression seeds
cargo nextest run -p holon-loro-testing > /tmp/refsplit-loro.log   # increments 5–6 only
```

**The gate is the HEADLESS keystone (`general_e2e_composed_pbt`) + the persisted
regression seeds.** The WINDOWED variant (`gpui_composed_windowed_loop`) carries a
**pre-existing RED** — `inv-watch-rows-match-ref` misses 4 forward-edge `fe-*` blocks
(the fe-* forward-edge gap, tracked separately), unrelated to this refactor — so it is
**excluded-with-reason** from the DONE gate until that fix lands. Do NOT chase it green
here. The one obligation: **Increment 4 (ui methods) must confirm the windowed failure
signature is UNCHANGED** — same 4 `fe-*` blocks, same invariant — not that it passes.
A *different* windowed signature after Inc 4 means the ui-fragment method push-down
perturbed focus/nav prediction and must be investigated before merge.

### Increment 1 — type extraction (pure moves)
Move type **definitions + their inherent impls** out of `reference_state.rs`:
`BlockState`/`LayoutBlockInfo` → `pbt/block_state.rs`; `ActiveEditor`/`CursorPosition`/
`NavigationHistory`/`OpenPinEntry` → `pbt/ui_types.rs`; `ClockState` →
`pbt/clock_state.rs`; `PeerRefState` → `pbt/peer_ref_state.rs` (staging stop before the
crate move in Inc 5). Update every importer directly (~40 files import via
`reference_state::…`, incl. `ui_actor_state.rs`, `action_actor_state.rs`,
`state_machine.rs`, `advice_expectation.rs`); **no long-lived re-export shims** (per
the no-old-code-paths directive; a re-export may exist only within the increment's own
commit if it keeps the diff reviewable, and must be gone at the increment's end).
- **DONE:** gate green; `reference_state.rs` contains no struct defs other than
  `ReferenceState`, `Resolved`, `TestEntityProfile`(+helpers).
- **Blast radius:** ~40 files, import lines only. **Risk: LOW.** Pure move; the one
  trap is `BlockState`'s doc-comment invariants (BTreeMap determinism) — move comments
  with the code.

### Increment 2 — split `reference_capabilities.rs` into `ref_caps/` (pure file split)
One file per trait cluster (§1 layout). No signature or body changes; `mod.rs` keeps
`reference_state_ref_caps()`, `CapProvider`, and the id-translation helpers
(`cap_id`, `parse_id`, `from_cap_region`).
- **DONE:** gate green; `reference_capabilities.rs` deleted; each `ref_caps/*.rs`
  < ~400 lines.
- **Blast radius:** 1 file → ~13 files; importers reference the module path, so
  `pbt/mod.rs` re-exports keep call sites untouched. **Risk: LOW.**

### Increment 3 — harness residue → `HarnessEnv`
Gather `runtime`, `wiring`, `cap_set`, `real_editor`, `interpreter` into
`HarnessEnv` (a field of `ReferenceState`); move `pre_startup_directories`,
`pre_startup_file_count`, `git_initialized`, `jj_initialized` into
`FileAdapterState`. Keep inherent accessor mirrors (`enable_loro()`, `caps_available()`)
on `ReferenceState`.
- **DONE:** gate green; `ReferenceState` has ≤ 10 top-level fields.
- **Blast radius:** field-path edits across pbt/ + slice harnesses (`frontend_slice`,
  `memory_slice`, `sql_slice`, `window_slice`, `loro_slice`, phased.rs sets
  `real_editor`). **Risk: LOW-MED** — `clock_feed`'s documented Clone-SHARES-the-cell
  semantics must not be disturbed (leave `clock_feed` OUT of HarnessEnv for now; it
  moves with the Loro ext in Inc 5, where its seam is documented).

### Increment 4 — method push-down (domain & ui)
Move single-fragment method **bodies** onto the fragment (`impl BlockState` /
`impl ReferenceDomainState`: `sorted_children_of`, `children_of`, `previous_sibling`,
`next_sibling`, `grandparent`, `text_block_ids`, `no_content_update_set`,
`has_blocks_profile`, `doc_uri_by_name`; `impl UITabState`/`UIUserState`:
`current_focus`, `has_focus`, `focused_entity`, `can_go_back/forward`, `current_view`).
`ReferenceState` keeps one-line delegators (residual transitions + `ref_caps/` call
them). Cross-fragment methods (§1 list) DO NOT move — that is the accept/reject
criterion for each method: *does the body read/write more than one fragment (or
harness)? then it stays on the root.*
- **DONE:** gate green; every method remaining in `refstate/mod.rs` provably touches
  ≥ 2 fragments (spot-check in review).
- **Blast radius:** internal to 3 files + `ref_caps/` forwarding targets.
  **Risk: MED** — judgment calls on borderline methods (`expected_focus_root_ids`
  reads ui only → could move to `UIUserState`, but it mirrors a SQL matview and is
  documented against `schema/matview_focus_roots.sql`; recommend it stays put to keep
  the SQL-mirroring oracles greppable in one place). Reviewer should audit the split
  list, not the mechanics.

### Increment 5 — Loro-private extension → `holon-loro-testing` (the co-location step)
1. Move `shadow_mesh.rs` → `crates/holon-loro-testing/src/shadow_mesh.rs`.
2. Move `PeerRefState` + new `LoroRefExt { peers, shadow_mesh, clock_feed }` into
   `crates/holon-loro-testing/src/ref_ext.rs`; port the **inherent** logic of the
   250-line `RefPeersMut for ReferenceState` impl (`reference_capabilities.rs:561-805`,
   incl. `merge_peer_blocks_into_primary`) into methods on `LoroRefExt` that take the
   shared data through **parameters shaped like the core's write interface**
   (`&mut BlockState` / callback), per §5.5's "the module hands intent; the core
   computes the merge".
3. `ReferenceState.loro: LoroRefExt` replaces the three loose fields; `ref_caps/peers.rs`
   keeps thin `RefPeers(Mut) for ReferenceState` impls (orphan rule: trait in pbt-core,
   type in integration-tests → impl must stay here) delegating to `LoroRefExt`.
4. `shadow_catch_up_primary` and `peer_modified_stable_ids` stay as root methods
   delegating in.
- **DONE:** gate green **including** `holon-loro-testing` tests and the keystone's
  `Loro;;UI` pinned case + the persisted SplitBlock/BulkExternalAdd regression seed;
  no `peers`/`shadow_mesh`/`clock_feed` identifiers left in `refstate/mod.rs` except
  the `loro` field and the two delegators.
- **Blast radius:** 2 crates; dependency direction is already correct
  (integration-tests depends on holon-loro-testing). **Risk: HIGH** — this is the one
  increment with real semantic surface: (a) the Lamport `clock_feed` Clone-shares-cell
  seam; (b) `merge_from_peer` ordering vs. the fi-tie/sort_key oracles
  (`composed_peer_sibling_order` history); (c) ShadowMesh deep-fork-on-Clone cost is on
  the proptest hot path. Mandatory review gate + a longer keystone soak
  (`PROPTEST_CASES` bumped) before merge.

### Increment 6 — decision point: open extension registry (§5.5 backlog (b))
Replace the hardcoded private-fragment fields (`loro`, `files`, `mcp`) with the typemap
registry so a new subsystem crate can register its private ref-state without editing
`ReferenceState`. **Not scheduled** — this is an architecture fork (typemap ergonomics
vs. plain fields; interaction with `Clone`-per-proptest-step cost; `Resolved` witness
plumbed through a typemap) that needs a Martin ruling with options laid out. Increments
1–5 are pure wins under either outcome and make the registry diff small if ratified.

### Non-goals (explicit)
- De-concretizing the 6 genuinely-residual transition files (orphan-anchor / assembler /
  boot-oracle residuals from the Phase 1a sweep) — separate track, unblocked but not
  required by this split.
- Any change to `holon-pbt-core/src/capabilities.rs` trait definitions.
- Splitting domain/ui/editor/focus into independently registrable modules — ratified
  out by §5.1/§5.5.

---

## 4. Risk register

| # | Risk | Increment | Mitigation |
|---|---|---|---|
| R1 | Iteration-order / determinism drift: `BlockState.blocks` BTreeMap ordering feeds sequence-number canonicalization; any accidental map-type or key change shifts every generated case and invalidates persisted regression seeds | 1,4,5 | Pure moves only; run the persisted seeds explicitly in the DONE gate |
| R2 | Editor↔blocks commit contract (ADR 0012 §5.1/§5.2): `blur_active_editor` / dirty-gating moved or split by mistake reintroduces the 2026-06-11 Full/Loro divergence family | 4 | Contract methods pinned to the composition root (§1 list); reviewer checks the list, CI keystone exercises it |
| R3 | Undo snapshot seam: `ActionActorState` stacks clone `BlockState` across the action↔domain boundary. Undo U1 (foundation) and U4 (split/join compound inverses) have **LANDED** on integration; the remaining collision is the **QUEUED U5 keystone-undo-rung stream**, which will touch `push_undo_snapshot` / `reference_state.rs` when spawned | 1,4 | Sequence Inc 1+2 (and ideally Inc 4's undo-adjacent method decisions) **before U5 spawns**, or hand U5's implementer the post-split layout so it targets the new fragment structure directly. No freeze needed — U1/U4 are already in; this is forward-coordination with a not-yet-started stream |
| R4 | Loro ext extraction changes peer-merge ordering semantics (fi-tie/sort_key oracles) or the shared-clock-cell Clone semantics | 5 | Port as inherent-method move, no logic edits; soak with raised case count; review gate |
| R5 | Merge conflicts with concurrent streams (links/marks oracle work, RowIdentity, undo) all landing in `reference_state.rs` | all | Small increments, land within a day each, sequence Inc 1–2 first (they *reduce* future conflict surface) |
| R6 | `Resolved` witness weakening: `remapped_doc_uris` must keep remapping exactly blocks + block_documents after fields move | 1,5 | Method stays on root; compile-witness (`Resolved`) unchanged; `exp3_unreconciled_split_is_caught` covers the under-reconciled case |
| R7 | `BuilderServices` impl accidentally fragmented — it must keep reading domain+ui+interpreter coherently in one impl | 4 | Move to `refstate/builder_services.rs` as a whole file, never split |

Cross-sub-state invariants to keep greppable in one place (do not scatter):
sequence canonicalization (`recanon_and_rebuild`), SQL-matview mirrors
(`expected_focus_root_ids`, sql-budget bookkeeping in `UITabState`), the
commit-point contract, and the shadow-mesh Lamport padding.

---

## 5. Increment → implementer tiering & blast radius

| Inc | What | Tier | Blast radius | Review gate |
|---|---|---|---|---|
| 1 | Type extraction | **mech-executor** (fully specified move list) | ~40 files, imports only | normal PR review |
| 2 | `ref_caps/` file split | **mech-executor** | 1→13 files, zero semantic | normal |
| 3 | HarnessEnv + files-fragment fields | **executor** (Sonnet-tier judgment) | pbt/ + 5 slice harnesses | normal |
| 4 | Method push-down | **executor**, with the stays-on-root list from §1 as hard spec; escalate to Opus only if borderline methods multiply | 3–4 files + ref_caps | reviewer audits the moved-method list against the ≥2-fragment rule |
| 5 | Loro ext → holon-loro-testing | **Opus executor** | 2 crates, semantic surface | **mandatory review gate + keystone soak before merge** |
| 6 | Open registry | not scheduled | — | **Martin ruling first** (options doc, per explain-options directive) |

Sequencing: 1 → 2 can run in parallel workspaces (disjoint files); 3 → 4 sequential
after 1; 5 after 4. Every increment is independently landable and independently
valuable — stopping after any increment leaves the tree strictly better.

### Execution mechanics (workspace / weave protocol)

Each increment runs in its **own fresh jj workspace cut from the CURRENT integration
tip**, and **lands (weaves) before the next *dependent* increment starts** — so each
dependent increment builds on already-integrated work, never on a sibling's unlanded
draft. Concretely:

- **Inc 1 ∥ Inc 2 in parallel workspaces is fine** (disjoint files: Inc 1 touches
  `reference_state.rs` + type-importers; Inc 2 touches `reference_capabilities.rs` →
  `ref_caps/`). BUT they overlap in `pbt/mod.rs` (module declarations + re-exports), so
  **whichever lands second MUST rebase-verify against the first's weave** — re-run the
  full DONE gate on the rebased tree, not just on its own workspace — before it lands.
  The `mod.rs` module list is the one shared edit point; treat it as the conflict
  surface and reconcile there.
- **Inc 3, 4, 5 each cut fresh from the tip AFTER their predecessor has woven.** No
  parallel execution across the 3→4→5 chain — they share `refstate/mod.rs` and the
  fragment structs.
- Land within a day each (R5): the longer an increment's workspace lags the integration
  tip, the more it conflicts with the concurrent links/marks, RowIdentity, and undo
  streams also editing `reference_state.rs`.
