# Session handoff — 2026-05-29 (KF-8 fix + parallel pre-work)

Context for resuming in a fresh session. The user was driving Phase 2/3 of the
ADR 0004–0007 componentization in a **separate session**; this session did
parallel pre-work + root-caused and fixed KF-8. Nothing here touched the user's
Phase 2/3 files.

## TL;DR — what's done and where it lives

| Work | Status | Artifact |
|---|---|---|
| KF-8 root cause + fix | ✅ FIXED & verified | worktree `.claude/worktrees/kf8-split-reparent` (UNCOMMITTED) |
| `EntityUri` double-scheme assertion | ✅ added | same worktree |
| H7 ordering-authority reconciliation | ✅ | `devlog/2026-05-29-h7-ordering-authority-reconciliation.md` |
| H8 watermark necessity decision | ✅ | `devlog/2026-05-29-h8-watermark-necessity-decision.md` |
| H11 ADR prose (Markdown not future) | ✅ | `docs/adr/0004-…:104` (main repo) |
| De-Loro the capability axis | ✅ green | main repo working copy (UNCOMMITTED) |
| Phase −1 baseline | 🟡 partial | `BASELINE.md` (KF-1…KF-9 + caveats) |

## KF-8 — THE headline result (a real production bug)

**Symptom:** `general_e2e_pbt_sql_only` `inv-blocks-match-ref/matview` Strict
failure under churn (`PROPTEST_CASES≥8`): a reparented block sits under its old
parent in SQL while the reference has the new parent.

**Root cause (NOT matview/split/Phase-8/rename — all ruled out):** a **double-
scheme URI bug** in the shared block-move path. Reparent (Indent / DragDropBlock
→ `move_block`) flows `move_block → move_to_position → BlockOrdering::place`.
`move_to_position` (`crates/holon-core/src/traits.rs`) built the URI with
`EntityUri::block(id)` where `id` is **already** `block:…`, producing
`block:block:…`. In SqlOnly, `place()` then runs `set_field parent_id` with id
`block:block:…` → `UPDATE … WHERE id = 'block:block:…'` matches **zero rows** →
reparent silently dropped. Loro mode dodges it (`write_position` moves via the
tree; `place()` returns before the doubled `set_field`) — hence SqlOnly-only.

**Proof:** tracer `[KF8-setfield] id=block:block:36fc9ccc… parent_id := r0--y…`.

**Fix (2 files, in the worktree):**
1. `crates/holon-core/src/traits.rs` `move_to_position`:
   `let uri = EntityUri::block(id);` → `let uri = EntityUri::from_raw(id);`
   (+ a comment explaining the hazard). `from_raw` accepts an already-schemed
   URI verbatim and still schemes a bare id.
2. `crates/holon-api/src/entity_uri.rs` `EntityUri::new`: a `debug_assert!` that
   `path` does not begin with a known scheme prefix
   (`["block:","doc:","file:","sentinel:"]`). Construction-time tripwire for the
   whole bug class; restricted to known schemes so synthetic ids
   (`default-main-panel::src::0`) and `block:{peer}:{counter}` don't
   false-positive. (User explicitly requested this assertion.)

**Verification** (`PROPTEST_CASES=12`, isolated `CARGO_TARGET_DIR=/tmp/holon-kf8-target`):
the matview divergence (KF-8 signature) drops from **2 in every pre-fix run AND
on base `70012b41` → 0** with the fix, deterministically. Assertion: 0
false-positives across the suite's heavy `EntityUri` construction.

**`sql_only` is still RED** — but only on **pre-existing, previously-masked**
failures the KF-8 panic used to short-circuit before reaching:
- `inv-value-fn-provider-arg-variance/vfn11` — **pre-existing, now unmasked
  (PROVEN, not a regression).** The runner runs `InvBlocksMatchRefMatview` at
  `invariant_runner.rs:234` but vfn11 at `:326`. Pre-fix the matview panic
  (Strict) aborted the case at 234, so vfn11 was never reached (vfn11=0 in all
  pre-fix runs AND base). Post-fix matview passes → the case reaches 326 → the
  pre-existing vfn11 failure surfaces. Both post-fix runs show matview=0,
  vfn11 present. It's a render-provider/focus_chain invariant, mechanically
  unrelated to `parent_id`; documented in project memory as the "vfn11/12/13
  needs a non-navigation render home" fixture follow-up. Fix it separately.
- KF-1 `seen_transitions_counter` (proptest-state-machine framework artifact on
  stale persisted seeds) — `BASELINE.md` KF-1.

Full investigation: `devlog/2026-05-29-sql_only-matview-reparent-divergence.md`
(in the worktree; mirror in main repo).

## NEXT STEPS (tomorrow)

1. **Land the KF-8 fix.** It's UNCOMMITTED in worktree `kf8-split-reparent` and
   `git diff` there is unreliable (worktree path quirk). The two changes are
   small — re-apply or `git add` them explicitly. Suggested commit:
   `fix(block-move): stop double-scheming the move URI (KF-8 SqlOnly reparent drop) + EntityUri::new guard`.
   Decide: keep the worktree, or port the 2-file change to the main line.
2. **`vfn11` is pre-existing + unmasked — already PROVEN** (runner ordering
   234 matview < 326 vfn11; see above). No further confirmation needed. Fix the
   vfn11/12/13 fixture as separate, known work ("needs a non-navigation render
   home") whenever you want `sql_only` fully green.
3. **De-Loro rename** (main repo, uncommitted, compiles green): commit as
   `refactor(capability): name the consolidator axis by mechanism, not Loro`.
   Boundary "Loro" survives only at `BlockCellRegistry::has_loro_backing()` +
   `CapabilityProfile::detect(loro_present)`. (Independent of Phase 2 files.)
4. **Phase 2/3 (user's track):** UIActorState extraction was committed
   (`1d2fd1bdef`); ReferenceState now has `.ui`. Continue per
   `~/.claude/plans/please-read-docs-adr-0004-composed-lantern.md` (status
   section is reconciled; H7/H8 pre-work done; residual Phase-8 debt items (a)
   move-vocabulary + (b) retire `assign_reference_sequences_canonical` still open).
5. **Phase −1 N≥5 sweep** still owed a clean re-run from a compiling tree (the
   2026-05-29 sweep was contaminated mid-run by the `.ui` refactor — see
   `BASELINE.md` contamination note; `org_create` result there is INVALID).

## Worktree / repro notes

- Worktree: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/kf8-split-reparent`
  (base `1d2fd1bdef` "Phase 3 — extract UIActorState"). All `KF8-*` eprintln
  probes have been removed; only the 2 real changes remain.
- Repro command (isolated target, no nextest 600s cap):
  `CARGO_TARGET_DIR=/tmp/holon-kf8-target PROPTEST_CASES=12 PROPTEST_MAX_SHRINK_ITERS=0 cargo test -p holon-integration-tests --test general_e2e_pbt general_e2e_pbt_sql_only -- --nocapture`
- The bug needs churn (`cases≥8`); no single replayable seed (proptest didn't
  persist one — timeouts/no-shrink). `cases=1` does NOT reproduce it.
- `.config/nextest.toml` hard-caps PBT tests at 600s; Full at `cases=8` times out
  under nextest — use `cargo test` for heavy case counts (see BASELINE caveat).
