---
id: 2026-08-12-only-drawer-string-compass-spine-invisible
date: 2026-08-12
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  `contributes-to` was only a drawer STRING, so the Compass spine was
  invisible to every edge query.
source_line: 722
---

## Bug

(D1.a contributes-to lane; found by dogfooding the Compass feature set (F5);
no automated test produced it) **`contributes-to` was only a drawer STRING,
so the Compass spine was invisible to every edge query.** `requires`
round-trips as a typed edge (junction → matview hydration → disk → cold
boot); `contributes-to` stayed in `block.properties`, was emitted by
`build_block_params` as a flat string param, and landed in the `properties`
JSON blob — unreachable from `block_links`, backlinks, or any reverse
closure, so "what advances goal X" could not be answered. Byte-stability
masked it: a surviving property renders identically to a projected edge.

## Root cause

secondary ORACLE, D1.a contributes-to lane, found by DOGFOODING the Compass
feature set (F5) — no automated test produced it: **`contributes-to` was
only a drawer STRING, so the Compass spine existed nowhere the system can
query it.** `requires`, the other arc direction of the same relation,
round-trips as a typed edge through `Block.requires` → the `block_requires`
junction → the `block` matview's hydrated JSON → disk → cold boot.
`contributes-to` had none of that: the parser left it in `block.properties`,
`build_block_params` emitted it verbatim as a flat string param, and it
landed in the `properties` JSON blob — invisible to `block_links`, to
backlinks, and to any reverse closure, so "what advances goal X" could not
be answered at all. Byte-stability hid it: a property that survives verbatim
renders identically to an edge that was projected, which is why
`org_store_org_round_trip.rs` was green over a `:contributes-to:` fixture
the whole time. COVERAGE primary: the concept had no field on `Block` and no
generator arm authored the drawer, so no sequence could reach a state where
the edge existed to be judged. ORACLE secondary: with the key living only in
`properties`, ref and SUT agreed — `inv-blocks-match-ref` compares the
property map, so it convicted nothing; no invariant could express "this key
belongs in a junction, not the blob". Missing piece: the model itself — an
`EdgeField` variant, so `EdgeField::ALL` carries the concept to every
projection site. FIXED in this lane: `EdgeField::ContributesTo` +
`Block.contributes_to` + the `block_contributes_to` junction + its agg
matview (synthesized from the descriptor registry, no hand-written DDL) +
the parse-boundary lift + the renderer rebuild; the positional edge-argument
lists on every create path became one `BlockEdges` value, because a fourth
positional `&[EntityUri]` next to two others was the shape that made this
class silent in the first place. Red-for-the-right-reason:
`block_params::tests::contributes_to_is_emitted_as_a_typed_edge_not_a_drawer_string`
with `contributes-to: String("m1")` sitting in the params blob and
`contributes_to` absent (`lane-logs/blockparams-RED.log`).)

## Missing piece

COVERAGE: the concept had no `Block` field and no generator arm authored the
drawer, so the edge could never exist to be judged. ORACLE: with the key
only in `properties`, ref and SUT agreed and `inv-blocks-match-ref`
convicted nothing — no invariant could express "this key belongs in a
junction, not the blob". Missing piece: the model — an `EdgeField` variant,
so `EdgeField::ALL` carries it to every projection site.

## Remedy

FIXED 2026-08-12 — `EdgeField::ContributesTo` + `Block.contributes_to` + the
`block_contributes_to` junction + its agg matview (synthesized from the
`EdgeFieldDescriptor` registry) + the parse-boundary lift (bare ids; the
legacy `none` sentinel parses to the empty set) + the renderer rebuild
(scheme stripped, sorted). The positional edge-argument lists on the create
paths became one `BlockEdges` value — a fourth positional `&[EntityUri]`
beside two others is exactly what made this class silent. RED:
`block_params::tests::contributes_to_is_emitted_as_a_typed_edge_not_a_drawer_string`,
`contributes-to: String("m1")` in the params blob with `contributes_to`
absent (`lane-logs/blockparams-RED.log`). Keystone: a `contributes_to`
generator axis on the org-file arm and a `SetEdgeField::ContributesTo` arm,
both judged by `inv-blocks-match-ref/matview`.
