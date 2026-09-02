---
id: 2026-09-02-org-write-back-halves-bold-markers-around-a-link
date: 2026-09-02
gap: ORACLE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Doubled emphasis such as `**[label](url)**` parsed into two identical `Bold`
  MarkSpans; the Loro Peritext attribute set holds only one of them, so
  write-back re-emitted the block as `*[label](url)*` on a page nobody edited.
---

## Bug

Found dogfooding a copy of the kitchen vault. A page the app only re-rendered
came back on disk with half its emphasis delimiters: an authored
`**[label](url)**` was written as `*[label](url)*`. The same happened for
`**plain**`, `//italic//`, `__under__` and `++strike++` — every doubled
delimiter form, whether or not a link sat inside it.

The loss is silent and one-way: nothing logs, and the next ingest reads the
degraded bytes as the new truth.

Evidence: `vault-diff.txt`, `bold-orig.txt` and `bold-copy.txt` in the lane
scratchpad
(`/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/1d3fdfe9-af2d-42a8-aecb-fbc009830160/scratchpad/`).

## Root cause

orgize parses `**x**` as bold nested inside bold. The emphasis arm of
`extract_inline_marks` (`crates/holon-org-format/src/inline_marks.rs:841`)
stripped both layers and recursed, so it emitted content `x` under **two**
`MarkSpan`s with the same range and the same mark.

That mark set has no representation in the store. Loro keeps marks as a
Peritext attribute set keyed by `(range, key, value)`
(`crates/holon-api/src/inline_mark.rs:243`), so both spans land on the same
attribute and one comes back. The renderer then emits one delimiter pair for
the one mark it is given.

Measured, before the fix
(`lane-logs/2026-09-02-probe.log`):

```
IN   "**bold**"
  content "bold"
  marks [MarkSpan { 0..4, Bold }, MarkSpan { 0..4, Bold }]
  OUT  "**bold**"  stable=true
```

The format-only round trip is `stable=true` — the duplicate survives a JSON
array. Only a real Loro document collapses it.

## Missing piece

**ORACLE (primary).** `render_marks_fixed_point_pbt` generated the triggering
shape routinely — with the fix reverted it fails on `"**h**"` at cycle 0 — but
its oracle compared only the settled *content* bytes and the emitted bytes.
Both were already correct. No assertion said the mark set must be one the store
can hold, so a duplicate-minting parse was green.

**ENVIRONMENT (secondary).** No holon-org-format test carries marks through a
Loro document, and every fixture in `crates/holon-app/tests/
org_store_org_round_trip.rs` — the file that exists to measure the store seam —
was mark-free. `vault_writeback_stability` over the vault copy reports 0
unstable files (`lane-logs/2026-09-02-vaultsim-before.log`) because it exercises
the format leg only.

## Remedy

Fixed in `crates/holon-org-format/src/inline_marks.rs`, both halves of the seam:

- Extract: when recursion yields a mark equal to the outer mark covering the
  whole inner text, `emit_with_literal_inner_delimiters` emits the node as one
  mark over the literal inner pair — content `*x*` under a single `Bold`.
- Render: `quotable_markup_spans` leaves a markup-shaped span alone when a
  styling mark of the same delimiter covers it exactly, so the mark's own
  delimiters restore the authored doubled form instead of quoting it to
  `*=*x*=*`.

The duplicate-free rule now lives on `MarkSpan`
(`crates/holon-api/src/inline_mark.rs:243`).

Covering tests:

- `holon-org-format::render_marks_fixed_point_pbt
  any_generated_store_state_reaches_a_fixed_point` — asserts every parse mints
  a duplicate-free mark set (closes the oracle gap).
- `holon-org-format::render_lossless_shapes
  doubled_emphasis_round_trips_byte_identically` — the doubled forms and their
  link nestings, byte-identical.
- `holon-app::org_store_org_round_trip
  inline_marks_around_a_link_survive_the_loro_text_seam` — drives a real
  `LoroBackend` and compares the mark set that comes back.
- `holon-app::org_store_org_round_trip
  inline_marks_around_a_link_survive_both_write_legs` — org → store → org bytes
  on the OrgIngest and Loro write legs.
