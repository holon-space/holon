---
id: 2026-08-19-cross-key-slot-clobbered-by-typed-edge
date: 2026-08-19
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A template block with a `{{slot}}` under one dependency spelling and a real
  block id under the other (`:REQUIRES: {{x}}` + `:BLOCKED-BY: real-dep`) drops
  the slot silently on the first write-back.
---

## Bug
Inside a `:TEMPLATE:` subtree declaring `:TEMPLATE_VARS: x`, a block authored as

```
:REQUIRES: {{x}}
:BLOCKED-BY: real-dep
```

parsed to `requires=[block:real-dep]` PLUS a carried property `REQUIRES="{{x}}"`,
and the FIRST org write-back rendered only `:REQUIRES: real-dep` — the `{{x}}`
slot vanished. A template rewritten to disk without its dependency slot can
never be instantiated, and no warning was emitted (a silent-data-loss
violation of the fail-loud rule).

Found by a verifier probe (`crates/holon-org-format/tests/zzprobe4.rs`, case
`q1`), i.e. outside an automated test. Lane `sw/bug-org-errors`, on top of the
landed template-slot-edges fix (d4c09b7a). Red evidence: `/tmp/orgerr-q1-red.log`
(the two promoted pins fail at `block.requires.is_empty()`); the raw probe red
is `/tmp/orgerr-q1-probe.log`.

## Root cause
`:REQUIRES:` and `:BLOCKED-BY:` are two org-drawer spellings of the SAME
`block_requires` edge. The parser processed each drawer key independently
(`crates/holon-org-format/src/parser.rs`, headline loop and src-block header-arg
loop): the slot-bearing spelling fell through `edge_ids() == None` and was
stored as a flat `REQUIRES` property, while the real-id spelling was lifted into
`block.requires`. At render, `drawer_properties()`
(`crates/holon-org-format/src/models.rs:928-941`) rebuilt `REQUIRES` from the
typed edge with an unconditional `result.insert("REQUIRES", …)`, clobbering the
carried-slot property inserted earlier via `entry().or_insert`. TWO writers over
one canonical drawer key; the typed-edge writer wins and the slot is lost.

By ruled design a mixed value in ONE spelling (`:REQUIRES: {{x}} real-dep`)
yields no edge and is carried verbatim (round-trips byte-equal). The cross-key
shape is the same mixed list split across two spellings, but the per-key parse
never recombined it.

## Missing piece
The dedicated PBT `template_slot_edges.rs` had a fixed-point oracle
(`every_edge_key_and_block_kind_reaches_a_slot_preserving_fixed_point`) that
WOULD have gone red on this case — but its case matrix only enumerated
single-key slots, never the cross-key `slot-under-one-spelling +
real-under-the-other` combination. The oracle was sufficient; the generated case
was absent. COVERAGE, not ORACLE.

## Remedy
Fixed. The parser now resolves the whole `:REQUIRES:`/`:BLOCKED-BY:` group as a
UNIT (`resolve_dependency_edge` in `parser.rs`): a group holding ANY slot
contributes NO typed edge and is carried whole under the canonical `REQUIRES`
key (authored order on the headline path; sorted for determinism on the
`HashMap`-backed src-block path), funnelling the cross-key shape into the
already-correct single-value mixed-list representation. This makes the
two-writers-for-one-key hazard structurally impossible: exactly one writer owns
the canonical key.

Case matrix extended in `template_slot_edges.rs` with
`a_cross_key_slot_and_real_id_survive_the_round_trip` (headline, both spellings ×
both orders),
`a_cross_key_slot_and_real_id_on_a_source_block_survive_the_round_trip`, plus
`a_mixed_list_in_one_value_yields_no_edge_and_round_trips` and
`slot_spelling_variants_at_the_edge_boundary` promoted from the probe. Green:
`/tmp/orgerr-q1-green.log` (17/17).

Keystone (`general_e2e_composed_pbt.rs`) does not reproduce: it generates
synthetic org and never authors template subtrees with cross-key dependency
drawers — the same generation gap this entry records. The dedicated PBT is the
right home (per the dedicated-PBTs-share-keystone-structure directive).
