---
id: 2026-08-21-place-all-no-ops-under-upstream-consolidator
date: 2026-08-21
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  BlockOrdering::place_all silently reordered nothing whenever Loro owned
  sibling order — it returned Ok(()) after writing only SQL sort_keys that the
  outbound projector immediately overwrote from the tree.
---

## Bug

Found by the `lsqdb-import` lane on 2026-08-21 while building its store
keystone for the LogSeq-DB import, outside any existing automated test. Three
hand-made blocks, no importer involved: `create_in_tree_batch` persists
`[parent, bbbb, cccc, dddd]`, `children(parent)` reads back `[bbbb, cccc,
dddd]`, then `place_all(parent, [dddd, cccc, bbbb])` returns `Ok(())` and
`children(parent)` is byte-identical to before. Every placement dropped — not
partial, not settle lag (reproduced after both `wait_for_loro_quiescence` and
`wait_for_cdc_quiescent`).

Reproduced independently in this lane:
`crates/holon-integration-tests/tests/place_all_restates_total_sibling_order.rs`
(red log `lane-logs/red-1.log`: `1 test run: 0 passed, 1 failed`).

## Root cause

The resolved `BlockOrdering` is `SqlBlockOperations`
(`crates/holon/src/core/sql_block_operations.rs:440`), a hybrid store, not
`LoroBlockOrdering` — provable from the fact that `create_in_tree_batch`
returned all-`true`, which `LoroBlockOrdering::create_in_tree`
(`crates/holon-app/src/loro_seams.rs:418`) can never do since it returns
`Ok(false)` unconditionally.

`SqlBlockOperations::place` routes through
`cell_registry.write_position` → `LoroBackend::update_block_position`, i.e. the
Loro tree, whenever Loro is on. Its `place_all` **override**
(`sql_block_operations.rs:516`) ignored that route entirely and did only a SQL
`sort_key` monotonic relabel via `set_field`. Under
`Consolidator::Upstream` the Loro tree owns the fractional index and the
outbound projector writes `sort_key` *from* the tree — so every relabel write
was overwritten unread and the whole call was a silent no-op. The SQL
projection was never at fault: `block_raw.sort_key` agreed with the unchanged
tree precisely because it is projected from it.

`BlockOrdering::consolidator()` (`crates/holon-core/src/block_ordering.rs:194`)
is the seam for exactly this, and its own doc says "order-decision call sites
should branch on this". `SqlBlockOperations` branched on it in four other
places (now lines 211, 665, 830, 1108) — but not in `place_all`, the one
method whose entire job is deciding order.

## Missing piece

No test ever called `place_all` under `Consolidator::Upstream`. The only
existing test of it,
`place_all_re_keys_a_legacy_unkeyed_block_into_its_requested_slot`
(`sql_block_operations.rs:2105`), runs in `Store` mode, where the relabel is
correct. The gap was self-concealing: the sole production caller, the org
re-ingest at `crates/holon-filesystem/src/file_sync_controller.rs:3748`,
duplicates the mode branch at its own level
(`if matches!(self.ordering.consolidator(), Consolidator::Upstream)` at
line 3637) and calls `place_all` only in the `else` (SqlOnly) arm. So prod
never reached the broken pair, no coverage was ever pulled toward it, and the
trait's documented contract was silently false for half its configuration
space — waiting for the first non-org caller (the LogSeq-DB import) to hit it.

Blast radius check: the org re-ingest total-reorder path named in `place_all`'s
own doc comment as its reason to exist was **not** broken in prod, because of
that controller-level guard. No second entry is warranted.

## Remedy

`SqlBlockOperations::place_all` now branches on `consolidator()`: under
`Upstream` it restates the order through `place` (the trait default's
predecessor-threaded loop, which the trait doc already declares correct for
tree-backed owners because `place` is idempotent and reads live neighbours);
the SQL monotonic relabel is kept for `Store`. Contract unchanged, no caller
weakened.

Pinned by `crates/holon-integration-tests/tests/place_all_restates_total_sibling_order.rs`,
which asserts both the total reorder and its idempotence under the default
(Loro/Upstream) harness wiring.

Left open deliberately: the controller's own mode branch at
`file_sync_controller.rs:3637` is now redundant — `place_all` is finally the
mode-agnostic seam it was documented to be, so both arms could collapse into
one `place_all` call. That is a behaviour change to the prod org path and is
out of scope for this fix.

## Verifier caveats (2026-08-21, carried for the follow-up)

- The Upstream arm inherits `place`'s residual silent no-op: `write_position` returns
  Ok(false) for blocks with no Loro tree node (synthetic render ids, unseeded rows) and
  `place` falls to the SQL sort_key path — inert under Upstream. place_all is total only
  for blocks actually in the tree. Pre-existing; identical to the controller's own arm.
- Subset semantics diverge between arms: the prod caller passes only Text children; the
  Store arm relabels the listed subset in place, the Upstream arm would hoist it to the
  FRONT of the sibling list. Harmless while the Upstream arm is prod-unreachable, but the
  open controller-branch collapse MUST keep the non-Text/foreign-subtree filters.
- The regression test asserts via BlockOrdering::children (the live tree — the order
  owner); it would not notice an outbound-projector failure to carry order into
  block_raw.sort_key. Scope limitation, covered elsewhere by matview invariants.
