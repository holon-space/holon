# sql_only `inv-blocks-match-ref/matview` divergence — re-parent projection gap

**Date:** 2026-05-29 (surfaced by the Phase −1 baseline sweep at `cases=8`)
**Status:** ✅ **FIXED** — double-scheme URI bug in the shared block-move path
(SqlOnly-only). Pre-existing (reproduces on base `70012b41`); NOT Phase-8, NOT
the de-Loro rename. Fix verified in worktree `kf8-split-reparent`.

## ROOT CAUSE + FIX (2026-05-29)

A reparent (Indent / DragDropBlock → `move_block`) flows `move_block →
move_to_position → BlockOrdering::place`. `move_to_position`
(`holon-core/src/traits.rs`) built the URI with `EntityUri::block(id)` where `id`
is **already** scheme-qualified (`block:…`), producing `block:block:…`. In
SqlOnly, `place()` then runs `set_field parent_id` with id `block:block:…`, so
`UPDATE … WHERE id = 'block:block:…'` matches **zero rows** — the reparent is
silently dropped and the block stays under its old parent. Proven by a tracer:
`[KF8-setfield] id=block:block:36fc9ccc… parent_id := r0--y…`. Loro mode dodges it
(`write_position` handles the move via the tree; `place()` returns before the
doubled `set_field`) — which is why it only ever showed in `sql_only`.

**Fix (2 files):**
1. `holon-core/src/traits.rs` `move_to_position`: `EntityUri::block(id)` →
   `EntityUri::from_raw(id)`.
2. `holon-api/src/entity_uri.rs` `EntityUri::new`: `debug_assert!` the path does
   not begin with a known scheme prefix (`block:`/`doc:`/`file:`/`sentinel:`) — a
   construction-time tripwire for this whole bug class.

**Verification** (`PROPTEST_CASES=12`, isolated target): the matview reparent
divergence drops from **2 in every pre-fix run (and on base `70012b41`) → 0**,
deterministically. Assertion: zero false-positives across the suite. `sql_only`
remains RED only on **pre-existing, previously-masked** failures the KF-8 panic
used to short-circuit: `inv-value-fn-provider-arg-variance/vfn11` (known fixture
follow-up — "vfn11/12/13 needs a non-navigation render home") and KF-1
(`seen_transitions_counter` framework artifact). Both unrelated to reparenting.

---

### Earlier note (pre-fix, superseded by the ROOT CAUSE above)
**Status:** RESOLVED as **PRE-EXISTING** — reproduces identically on base `70012b41`
(the commit before Phase 8). NOT a Phase-8 regression, NOT the de-Loro rename.

## VERDICT (vs-base, 2026-05-29)

Ran `general_e2e_pbt_sql_only` at `PROPTEST_CASES=12` on a clean worktree of base
`70012b41` (isolated target). It **FAILED with the identical divergence** in 295 s:
`block:6674bb01…` nested under `block:r0--y-sx4pk-i---8r-462` in the reference,
top-level under `ref-doc-0` in matview/SQL — same parent_id reparent gap, same
nest-target as HEAD. The `inv-blocks-match-ref/matview` body + parent-comparing
`compare_block_fields` are byte-identical at base, so the comparison is valid.

→ **Pre-existing block-sync reparent/move-projection bug.** It is NOT introduced
by Phase 8 (whose `sql_block_operations` changes were read-side ordering only)
and NOT by the de-Loro rename. It is a known-RED baseline condition (see
`BASELINE.md` KF-7), not a blocker for Phase 2. Root-cause/fix is separate work.

## Symptom (precise)

`general_e2e_pbt_sql_only` fails `inv-blocks-match-ref/matview` (Strict) at
`PROPTEST_CASES=8`. Exactly **one block** diverges (8-block doc):

| block | reference `parent_id` | matview/SQL `parent_id` |
|---|---|---|
| `block:8f071457-2dba-402f-b3b2-441f7fd8809d` | `block:r0--y-sx4pk-i---8r-462` (nested) | `block:ref-doc-0` (top-level) |

It is a **`parent_id` (re-parent / nesting) divergence**, NOT an ordering or
`sort_key` issue.

## Localization

- The `/matview` body **retries for 5 s** (`retry_until_ok`) before failing →
  not CDC lag; the matview stayed diverged. The matview is a CDC-fed view of
  `block_raw`, so **the write side (`block_raw`) holds the wrong parent** — the
  re-parent never reached SQL.
- The Strict `/block_raw` invariant compares **`{content, properties}` only —
  not `parent_id`** — so a re-parent gap is invisible to it; `/matview` is the
  only store check that catches `parent_id`.
- Mode: **SqlOnly** (no Loro). The move/re-parent goes through the SQL write
  path directly (`sql_block_operations` `update_in_tree`), so the gap is in the
  SqlOnly move-projection, not the Loro→SQL projector.

This matches the long-documented block-sync reparent/move bug family (memory:
"split block messes up", update-churn #7, Loro↔SQL projection).

## Why it wasn't in BASELINE.md

BASELINE's sql_only entry only recorded KF-1 (the `seen_transitions_counter`
framework artifact) because the green/baseline runs used `PROPTEST_CASES=1`,
which never reaches the deep multi-step sequences (split + re-parent under
churn) that trigger this. It needs `cases≥?` and several steps.

## NOT caused by the de-Loro rename

The renamed files were modified at 18:28–18:29; this divergence failed on
pre-rename code at 18:13 and 18:23 (sql_only run1/run2). The rename is a pure
behavior-preserving variant/method rename and `cargo check` is green.

## Phase-8 attribution: open

Phase 8 (`8c9b6f19b2`) was **not** purely sort_key — it reworked the projection
path, incl. `sql_block_operations.rs` (+216, the `update_in_tree` parent/position
routing), `loro_sync_controller.rs`, `queryable_cache.rs` (+65), and
`SnapshotBlock` (sets `parent_id`). So a re-parent-projection regression *could*
have been introduced in Phase 8 — cannot be ruled out from the diff.

**Blocker:** the `cases=8` run **timed out** (600 s hard cap, `.config/nextest.toml`)
before proptest could shrink/persist — so there is **no replayable minimized
seed** (`general_e2e_pbt.proptest-regressions` unchanged). Without a seed the
vs-base (`70012b41`) comparison can't target the failing case.

## Attribution method (done)

Exact-seed replay was infeasible — the failure needs churn (a deep random
sequence), and proptest persisted no minimized seed (the run failed mid-shrink).
So attribution was done by **same-config reproduction on base**: `cases=12` on
`70012b41` reproduced it → pre-existing. (HEAD took 1663 s to hit it, base 295 s
— both reproduce; the time difference is just where in the random sequence the
churn lands, not significant.)

## Root-cause progress (2026-05-29, this session)

The "reparent" is actually a **SplitBlock at position 0** divergence — narrowed
from the `cases=12` capture log:

- `[SplitBlock-presplit] target=block:r0--y… position=0`. At split time
  `block:r0--y…` is a child of `block:ref-doc-0` (both `block_raw` AND `block`
  matview agree; sequence=2, siblings = block:1983fbd3, v, -b-t-oy4vc, r0--y,
  xpu074 — all under ref-doc-0).
- **Prod** `BlockOperations::split_block` (`holon-core/src/traits.rs:745`):
  `parent_for_split = block.parent_id()` = `ref-doc-0`; creates the new block as
  a **sibling** after `r0--y…`. Diag: `parent=block:ref-doc-0 after=block:r0--y…
  wrote_create_via_cell=false` (SqlOnly → SQL-direct create path, lines 868+).
  This is self-consistent and correct for r0--y…'s real parent.
- **Reference** `ReferenceState::split_block` (`reference_state.rs:1300`):
  `new_block` parent = `original.parent_id` (line 1305/1322) — i.e. also the
  original's parent. No position-0 special case; `recanon_and_rebuild` only
  touches sequences, not parents.
- **Paradox**: both impls copy the *original's* parent, so for a split of
  `r0--y…` (parent ref-doc-0) they should BOTH yield ref-doc-0 — yet the
  divergence dump shows the new block under `r0--y…` on the reference side and
  under `ref-doc-0` on the SQL side.

### Open candidates (need a deterministic repro to disambiguate)
1. **Multi-tick extraction artifact** — the `matview (normalized, 8 blocks)`
   dump appears 22× in the log; the first-match extraction may have paired a
   matview list from one tick with a reference list from another. RE-EXTRACT the
   single divergence at the FIRST failing tick (right after the split,
   ~line 3253) before trusting "reference says r0--y…".
2. **A later transition reparents in ref only** — e.g. a chord `SplitBlock →
   MoveUp → Indent` (the ref code at :1333 explicitly mentions this chord). An
   Indent of the new block would reparent it under its preceding sibling
   (`r0--y…`) in the reference; if prod's indent didn't propagate in SqlOnly the
   SQL side keeps ref-doc-0. **This is the strongest hypothesis** — the real bug
   would then be in **Indent/MoveAfter projection**, not split.
3. **Effective-target mismatch** — ref and SUT split different blocks via id
   resolution (less likely: same `block_id` field drives both).

### DECISIVE re-extraction (single tick, right after split-1)
Re-extracted the FIRST failing tick (not conflated): at that tick
`block:41dd545f` (the split block) has parent **`r0--y…` in the REFERENCE** and
**`ref-doc-0` in the matview/SQL**, while `r0--y…` itself is `ref-doc-0` on both
sides. So candidate #1 (multi-tick artifact) is REFUTED — the reference really
places the split block as a **child of the split target**, prod as a **sibling**.

### Contradiction to resolve (the actual open question)
Outliner semantics say **sibling is correct** (Enter splits into a sibling, not a
child) → **prod is right, the reference oracle is wrong** here. BUT the reference
code appears to already do sibling:
- `RefBlockTreeMut::split_block` (`reference_capabilities.rs:251`) just delegates
  to `ReferenceState::split_block` (`:253`).
- `ReferenceState::split_block` (`:1300`) sets the new block's parent =
  `original.parent_id` (`:1305`,`:1322`) — i.e. a sibling.
- `recanon_and_rebuild` (`:1433`) only reassigns sequences + profiles; it does
  **NOT** reparent.

So the code says sibling, the log says child. Unresolved candidates:
1. **Effective-target mismatch** — the reference's `block_id` for this
   SplitBlock resolves to a block whose parent is `r0--y…` (a child of r0--y…),
   while the SUT clicked/split `r0--y…` itself. Then ref new block parent =
   r0--y… (correct sibling of THAT child) and prod new block parent = ref-doc-0
   (sibling of r0--y…). This now looks like the **leading** hypothesis (it
   explains the child-vs-sibling split cleanly). Would be a SUT id-resolution /
   click-target bug, NOT a prod write bug. Caveat: it should also produce a
   *content* divergence (different block emptied) — only 1 block diverged, so
   verify the content angle.
2. A code path other than the three above mutates the new block's parent.

### Next step (blocked on compile)
A **deterministic instrumented repro** is required to resolve the contradiction:
in SqlOnly, build a doc with a nested block, drive split-at-0 on a known target,
and log BOTH the reference `block_id`+resulting parent AND the SUT's
resolved/clicked target. That pins target-mismatch (harness) vs a parent-write
bug (prod). **Blocked right now:** the working copy does not compile (in-flight
`reference_state` `.ui` extraction from the parallel Phase 2/3 session), so no
build/verify is possible. Resume once the tree compiles.

**Do NOT ship a speculative fix** (per project memory's repeated lesson): the
code-vs-log contradiction means any edit now would be a guess. The fix target
(reference oracle vs prod vs SUT resolution) is not yet proven.

### Update (later same session) — contradiction sharpened, NOT yet resolved
Read the CURRENT `ReferenceState::split_block` (now line ~1237 after the parallel
`.ui` extraction shrank the file): **unchanged** — new block parent =
`original.parent_id` (sibling). Re-walked the whole reference path
(`split_block_apply_to_ref` → trait `split_block` → `ReferenceState::split_block`
→ `recanon_and_rebuild` → `assign_reference_sequences_canonical`): **none
reparent**. Also confirmed via the `[inv-sql-budget]` markers that the divergence
dump sits inside **SplitBlock's own** post-transition invariant sweep (prev
marker = SplitBlock; next = the following case's StartApp), with **no
intervening transition**. So: code says sibling, log says the reference holds a
child, at SplitBlock's own check. Hard contradiction — unresolvable by reading.

**Decisive probe (ready to add when the tree is stable):** one `eprintln!` at the
end of `ReferenceState::split_block`, right after `blocks.insert(...)`:
```rust
eprintln!("[KF8-ref-split] block_id={} original.parent={} new_id={}",
    block_id.as_str(), parent_id.as_str(), new_id.as_str());
```
Then run `PROPTEST_CASES=12 cargo test -p holon-integration-tests --test
general_e2e_pbt general_e2e_pbt_sql_only -- --nocapture` (isolated target).
- prints `parent=ref-doc-0` but the invariant later sees `r0--y…` → something
  BETWEEN split and the check reparents the *resolved* reference view
  (suspect: `with_resolved_doc_uris`/`remapped_doc_uris`, or a focus side-effect)
  → the bug is in the comparison/remapping, NOT split_block.
- prints `parent=r0--y…` → split_block IS assigning a child despite the code
  reading as sibling (then re-read very carefully / check `original` identity).

**Could NOT run it this session:** the working copy is being live-edited
(`reference_state` `.ui` extraction); it compiled briefly then broke again
(`E0609 focused_entity_id/open_pins/navigation_history/active_editor` — fields
moving to `.ui`). Adding a probe to a file under active edit collides and can't
be verified, so the probe was added then **reverted**. Resume when the tree is
stable for more than a moment.

### Aside: a cheap permanent guard worth adding regardless
Add a `parent_id` facet to the Strict `/block_raw` invariant
(`blocks_match_ref.rs` `InvBlocksMatchRefBlockRaw`, currently `{content,
properties}` only). It would catch any write-side reparent gap directly on the
write table — independent of the matview and of this specific repro.
