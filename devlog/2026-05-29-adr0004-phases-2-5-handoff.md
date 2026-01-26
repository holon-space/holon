# Handoff — ADR 0004–0007 componentization, Phases 2–5

**Session date:** 2026-05-29. **Plan:** `~/.claude/plans/please-read-docs-adr-0004-composed-lantern.md` (read it first — it has the full phase DAG + per-phase detail).

## TL;DR of what landed this session

All work is **PBT-reference-model fragment extraction** in `crates/holon-integration-tests/src/pbt/`. Each phase splits cohesive state out of the monolithic `ReferenceState` into a typed fragment, so a reduced wiring can drop the fragment instead of carrying dead fields (ADR 0004 tiering).

| Phase | What | Fragment / change | Status |
|-------|------|-------------------|--------|
| 2a | Tier-1 domain re-home | `ReferenceDomainState` (`reference_domain_state.rs`): block_state, layout_blocks, profile_block_ids, active_profiles, render_expressions, seed_profile, block_operations. `ReferenceState.domain`. | DONE, verified |
| 2b | Domain invariants | NEW `inv-no-parent-cycles`, `inv-source-language-iff-source` (bodies + registry, tagged `[BlockTree,TursoProjection]`). Documented that "refs resolve" / "ordered list" are already covered. | DONE, verified |
| 3 | Tier-3 UI actor | `UIActorState` (`ui_actor_state.rs`): navigation_history, open_pins, next_history_id, next_pin_ts, focused_entity_id, focused_block, focused_cursor, current_view, expanded_toggles, drawer_open, active_editor. `ReferenceState.ui`. | DONE, verified |
| 4 | MCP + Action actors | `MCPServerActorState` (active_watches) + `ActionActorState` (app_started, next_doc_id, last_transition_kind, undo_stack, redo_stack). `ReferenceState.mcp` / `.action`. | DONE, verified |
| 4b | Per-actor invariants | **Documented coverage, no new invariant** (user decision). MCP delta-correspondence is redundant with `inv-active-watches-match-ref` + `inv-watch-rows-match-ref`; action undo/redo-availability would be **vacuous** because the undo subsystem is dormant (`push_undo_snapshot` is a no-op — `SqlOperationProvider` returns `irreversible()` for all ops). Comment block in `registry.rs`. | DONE |
| 5 (mechanical) | Org/Md file-state | `OrgMarkdownFileState` (`org_markdown_file_state.rs`): the `documents` doc_uri→filename map. `ReferenceState.files`. ADR-0009 `Document.filename` prose **deferred** (decide with user). | DONE, verified |

`runtime` / `variant` / `interpreter` / `peers` deliberately stay on the `ReferenceState` facade (harness infra, per plan).

## The playbook (use this for Phases 6+)

1. **Compiler-driven re-home, NOT blanket ast-grep.** The moved field names collide with other structs (`UiState.focused_block`, `TestEnvironment.current_view`/`documents`, engine accessor methods `app_started()`/`current_view()`) and a name-based ast-grep corrupts those. Instead: create the fragment, move fields, add the facade field, fix the constructor → compile → the compiler flags **exactly** the `ReferenceState` accesses (`E0609 no field`, plus `E0615` where a same-named accessor method shadows the removed field).
2. **Column-precise insertion script.** Parse `cargo check` output for `(field, file, line, col)`, insert the prefix (`domain.`/`ui.`/`action.`/`mcp.`/`files.`) at each col, processing cols right-to-left per line, asserting the char before is `.`. (Phase 2a used ast-grep because there were zero collisions; 3/4/5 used the col script — it handles both E0609 and E0615.) Watch for residuals in separate **test-target** binaries (`tests/*.rs`) and in `declare_pbt_slice!` **macro bodies** (`slice.rs`) — these surface only on a second compile after the lib is clean.
3. **Verify** against `BASELINE.md`: `cargo check` clean, then `org_create_ordering_pbt_full` (fast gate, ~75s) + `general_e2e_pbt` Full. Use **`PROPTEST_CASES=1 PROPTEST_MAX_SHRINK_ITERS=0 cargo test ... --test general_e2e_pbt -- --exact general_e2e_pbt`** — NOT nextest (nextest has a 600s cap that kills the ~530s Full run; and default `cases:8` + shrinking shrink-loops into timeout). Gate = "no NEW deterministic failure" vs the KF list, not zero failures.
4. These re-homes are **value-preserving** (`.frag.field` reads the same data) — if it compiles + `org_create` passes, behavior is unchanged.

## BASELINE / known failures (do NOT attribute to a change)

`BASELINE.md` (root) has KF-1..KF-9. Most relevant: **KF-1** sql_only `seen_transitions_counter` (deterministic, framework replay artifact); **KF-8** sql_only intermittent Strict invariant races under stale-seed replay (`inv-focus-roots` / `inv-blocks-match-ref/matview`); **KF-9** Full flaky at `PROPTEST_CASES=1` (~2/5). `general_e2e_pbt_sql_only` is **RED at baseline** — don't expect it green. The proper N≥5 flakiness sweep is still owed.

## jj / worktree state — IMPORTANT

- **Phases 2a/2b/3 are committed in the MAIN workspace** (`/Users/martin/Workspaces/pkm/holon`): Phase 3 tip was `1d2fd1bd` (hashes may have moved — anchor by commit description).
- **Phases 4 + 4b + 5 live in the jj workspace `phase4-actor-states`** (`.claude/worktrees/phase4-actor-states`), created via the `WorktreeCreate` hook (`jj workspace add`). At handoff the stack tip is Phase 5 `e0a407bf` (`refactor(pbt): Phase 5 mechanical — extract OrgMarkdownFileState`). **These commits are NOT yet integrated into the main line** — they need rebasing/merging into wherever the user's main bookmark ends up. They're in the shared jj store, visible from both workspaces.
- **Parallel work is happening concurrently** in the main workspace (capability/`Consolidator` rename, H7/H8 devlogs, ADR-0004 edits, BASELINE updates). Per user instruction this was bundled into the Phase 2a/2b/3 commits untouched, and Phases 4–5 were moved into the worktree to isolate from it.
- **Concurrency hazard (hit this session):** the parallel session's repo-global jj ops rewrote/rebased my Phase 4/5 commits and triggered a `reconcile divergent operations`, leaving Phase 5 as a **divergent** change (`rzvszvkx/0` complete vs `rzvszvkx/2` a 3-file partial). Resolved by `jj edit <complete-commit>` then `jj abandon <partial>`. **Lesson: keep doing actor-extraction in an isolated worktree, and avoid running jj in two sessions on the same repo simultaneously.** If divergence recurs, the complete commit is the one whose `jj diff --stat` shows all ~26 files.

## What's next (DAG)

Phases 6, 7, 9, 10, 11 are all downstream of 5; with 3+4+5 done, **Phase 6 is now unblocked**:

- **Phase 5 leftover:** decide **ADR 0009 `Document.filename`** content with the user, then write the ADR + reconcile the `OrgMarkdownFileState` shape if needed (mechanical split already done; current shape = the whole doc_uri→filename map in the fragment, doc-identity = its keyset, undo-persistence moot while undo is dormant).
- **Phase 6** — finalize actor split: DSL-parser tier decision; per-tab vs per-user split. Depends 3,4,5.
- **Phase 7** — Wiring manifest + validity-rules table + Wiring PBT (drives DI + PBT). Then 9 (per-adapter schema registration), 11 (Todoist → Tier 2b), 10 (Loro→Turso bridge).
- **Open, independent of the DAG:** deviation #5 — retire `assign_reference_sequences_canonical` → `after`-based generators (still present in 8 files; risky, pokes cluster-#6 ordering flake). And the owed Phase −1 N≥5 baseline flakiness sweep.

## Gotchas

- `engram` hook mangles symbol names in `grep`/`rg` output to `n` — read files directly or use `cargo check` output for symbol-accurate info.
- The `WorktreeCreate` hook runs `cargo check --workspace` first and refuses to create a worktree if the tree doesn't compile.
- Registry has count-sensitive tests: `registry_size_matches_audit` (currently 40), `phase5_editor_loro_picks_up_expected_count` (range bumped to 10..=16), `body_ids_match_registry_ids` (hardcoded id list). Update all when adding/removing invariants. (Note: registry_size 37→38 and phase5 ≤15→16 were pre-existing drifts corrected in 2b.)
