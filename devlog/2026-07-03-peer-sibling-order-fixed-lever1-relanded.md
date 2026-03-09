# Peer-sibling-order fixed + lever 1 re-landed: keystone GREEN at `sequential 1..40`

**Worktree / jj workspace:** `composed-pbt-boot-parallelism`. Continues
`devlog/2026-07-03-composed-pbt-convergence-settle-landed.md` (handoff:
`scratchpad/handoff-peer-sibling-order-fix.md`). Uncommitted — commit only when asked.

## Deliverable

`general_e2e_composed_pbt` flipped `sequential 1..8` → **`1..40`** and validated **GREEN at
`PROPTEST_CASES=16`** (557s). Lib slice suite 158/158, holon-loro 124/124, gpui/tui/holon
`cargo check` clean. The cc regression seed appended by the first failing 1..40 run now
replays GREEN and was kept as a free regression case.

## Fix 1 — prod: tied fractional indexes never reached distinct SQL `sort_key`

Concurrent peer creates at the same position mint the SAME Loro fi (jitter=0). Loro breaks
the tie internally by op id, but the projected `sort_key` string carried only the fi → both
`block_raw` rows tied → SQL fell back to `ORDER BY … id` (random from the user's PoV, and
divergent from the Loro authority).

`effective_sibling_sort_keys` (`holon-loro/src/loro_backend.rs`): tied runs get a
`.<pos:06x>` suffix in `tree.children()` (true) order; `.` (0x2E) sorts below every fi hex
char, so suffixed keys keep their place vs distinctly-keyed siblings. Applied in BOTH
projection paths: `snapshot_blocks_from_doc_settled` (outbound projector) and
`block_sort_key` (org-scan order writeback via `live_sort_key`).

Deterministic regression guard (~2.3s): `peer_merge_sibling_order_sql_matches_loro`
(`pbt/frontend_slice/structural_pbt.rs` teeth) — SQL `sorted_children` must equal Loro
`loro_children_of` after a two-peer concurrent create + sync.

## Finding — the handoff's oracle plan was falsified by two diagnostics

The handoff assumed Loro's tied-fi order is *insertion (wall-clock creation) order*. Two
deterministic diags falsified that:

1. Higher peer id creating FIRST still sorts LAST at equal lamport → order is peer-id, not
   creation order.
2. Lower peer id creating after 5 unrelated text updates (lamport bump) sorts AFTER a
   higher-peer base-lamport create → lamport dominates.

So the tie order is op-id order **(lamport, peer id)**. A peer's lamport is a function of
the global doc's full op-atom history at AddPeer time plus per-character text-op spans —
no oracle can predict it without totally replaying the SUT (a shadow-LoroDoc oracle dies on
the AddPeer snapshot import). This kills both "model insertion order" and "model lamport".

## Fix 2 — oracle: verified SUT-adoption (the id-reconcile pattern, extended twice)

Where the SUT mints CRDT-arbitrary values, the oracle adopts the observed value instead of
predicting it (exactly like synthetic→real uuid reconcile). Two instances:

- **Sibling order**: `merge_peer_blocks_into_primary` (`pbt/state_machine.rs`) stamps
  peer-created blocks `max sibling seq + 1…` (models Loro's append — the deterministic
  part); `adopt_observed_peer_sibling_order` (`pbt/composed/harness.rs`, run_report)
  permutes ONLY `block:peer-*` siblings within their existing sequence slots to the
  observed `loro_children_of` order, and only when the whole group is observed.
- **Concurrent text merges** (second latent bug, disclosed by the first 1..40 run):
  `loro_merge_text` models the merge with synthetic peer ids over fresh docs, so its RGA
  interleaving can invert the real one (`"daabdaaa"` vs `"daaadaab"` — same char multiset).
  `BlockState::crdt_merged_content` records each model-merge result;
  `adopt_observed_crdt_merged_content` adopts the SUT's (Loro-authority) content **iff**
  the oracle block still holds the recorded merge (a later modeled write self-invalidates
  the entry) **and** the SUT content is char-multiset-equal (dropped/duplicated/foreign
  characters still fail loudly). Only the interleaving is ceded to the CRDT.

Teeth preserved: set-equality, causally-ordered sibling order, peer-group position after
existing children, and the cross-store agreement SQL == Loro (independent of the oracle,
via `inv-live-children-match-ref` + the new guard — which is where the actual prod bug
lived).

## Files touched

- `crates/holon-loro/src/loro_backend.rs` — `effective_sibling_sort_keys` + both call sites.
- `crates/holon-integration-tests/src/pbt/state_machine.rs` — merge stamping + merge-result
  recording.
- `crates/holon-integration-tests/src/pbt/reference_state.rs`,
  `reference_domain_state.rs` — `BlockState::crdt_merged_content` (+ remap + ctor sites).
- `crates/holon-integration-tests/src/pbt/composed/harness.rs` — the two adoption fns,
  called from the default `run_report`.
- `crates/holon-integration-tests/src/pbt/frontend_slice/structural_pbt.rs` — regression
  guard test.
- `crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs` — `1..8` → `1..40`.
- `…/general_e2e_composed_pbt.proptest-regressions` — one cc seed, now green, kept.

## Repro / validation commands

`bash -c 'PROPTEST_CASES=16 cargo nextest run -p holon-integration-tests --test
general_e2e_composed_pbt --no-capture | tee /tmp/run.log'` (always `bash -c '… | tee'`;
nu's `out+err>` redirect false-greens).
