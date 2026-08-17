---
id: 2026-08-04-when-intermediate-ancestor-gains-via-deep
date: 2026-08-04
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  When an INTERMEDIATE ancestor gains `Page` via `convert_block_to_page`, a
  deep descendant X re-homes to the new page's document while its own
  `parent_id`/`tags` stay UNCHANGED — so a queued edit for X routed to the OLD
  doc drains on the cheap path and re-renders X's subtree into the OLD file
source_line: 776
---

## Bug

(architecture lane, Option-C evidence — red-first repro of the stale-delta
write-back residual hole, options doc §2.3) **When an INTERMEDIATE ancestor
gains `Page` via `convert_block_to_page`, a deep descendant X re-homes to
the new page's document while its own `parent_id`/`tags` stay UNCHANGED — so
a queued edit for X routed to the OLD doc drains on the cheap path and
re-renders X's subtree into the OLD file** (`file_sync_controller.rs:3858`
gate and `:3872` authority re-check both compare only the block's own
parent/tags vs the cache, never the block's owning document vs the routed
`doc_id`; reseed skipped, removal veto bypassed, `last_projection` stamped).
Reproduced deterministically at the FileSyncController seam:
`crates/holon-orgmode/tests/sync_controller_mutation_pbt.rs`
`intermediate_ancestor_writeback_hole::convert_leaks_deep_descendant_edit_into_old_doc`
= RED (X's edit lands in P_a's `test.org`);
`::divergence_self_heals_after_ancestor_reseed` = GREEN — TRANSIENT: once
the re-homed child's own structural delta drains, P_a reseeds and the
removal veto grounds X's absence as a move via the authority
(`owning_file_of` reads authority, not disk).

## Root cause

secondary ORACLE: architecture-read + red-first repro of the stale-delta
write-back residual hole
(`~/.claude/plans/stale-delta-redesign-options-2026-08-04.md` §2.3) —
`render_with_cache`'s cheap-path gate (`file_sync_controller.rs:3855`) and
its authority re-check (`:3866-3872`) compare only the block's OWN
`parent_id`/`tags` against the doc cache, never whether the block still
belongs to the ROUTED `doc_id`; when an INTERMEDIATE ancestor gains `Page`
via `convert_block_to_page`, a deep descendant X re-homes to the new page's
document with its own `parent_id`/`tags` UNCHANGED, so a queued edit for X
routed to the OLD doc (T1) drains on the cheap path (T2) and re-renders X's
subtree into the OLD file (reseed skipped, removal veto bypassed,
`last_projection` stamped). The keystone HAS both `BlockToPage` and
block-edit transitions so the interaction is generatable, but it settles to
CDC quiescence between every transition — collapsing prod's unbounded async
delta queue so the two deltas never interleave in-flight, the exact "async
races the settle masks" ENVIRONMENT case. Reproduced deterministically at
the FileSyncController seam in
`crates/holon-orgmode/tests/sync_controller_mutation_pbt.rs`
(`intermediate_ancestor_writeback_hole::convert_leaks_deep_descendant_edit_into_old_doc`
= RED: X's edit lands in P_a's `test.org`;
`::divergence_self_heals_after_ancestor_reseed` = GREEN: transient,
self-heals once the ancestor's own structural delta reseeds P_a, grounding
X's removal as a move via the authority). Verdict: TRANSIENT divergence, not
durable data loss. Secondary ORACLE: no invariant asserts single-document
ownership DURING in-flight deltas, so even a generated race would self-heal
before the fixed-point oracle runs. Remedy = Option-C redesign evidence
(make the qualification decision unrepresentable), NOT a point fix)

## Missing piece

The keystone HAS `BlockToPage` + block-edit transitions, so the interaction
is generatable — but it settles to CDC quiescence between every transition,
collapsing prod's unbounded async delta queue; the hole needs X's edit and
the convert IN FLIGHT together, an interleaving the settle makes
unreachable. Missing piece = a keystone mode that does NOT settle between a
paired convert+edit (or injects a controlled queue delay) so cross-doc
deltas interleave. Secondary ORACLE: no invariant asserts single-document
ownership DURING in-flight deltas, so even a generated race would self-heal
before the fixed-point oracle runs.

## Remedy

OPEN 2026-08-04 — evidence lane for the ratified Option-C redesign
(`docs/Plans/option-c-holder-design.md`): the remedy is making the
qualification decision unrepresentable (declared `home_by` holder), NOT a
point fix; the RED test is the Inc-0 red-log and stays `#[ignore]`d until
Inc 2 cuts over.
