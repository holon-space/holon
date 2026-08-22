---
id: 2026-08-22-importbase-serialization-is-not-byte-stable
date: 2026-08-22
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  Importing the SAME LogSeq graph twice serializes to different bytes, because
  a `_logseq_raw/*` property holding a nested map lands in a `HashMap`-backed
  `Value::Object` whose emission order follows the per-process hash seed.
---

## Bug
The W0 LogSeq-DB round-trip spike (lane `lsqdb-import`, 2026-08-22) asserted
that a re-encoded copy of the fixture imports to a byte-identical
`ImportBase`. It failed — but so did the control: importing the committed
fixture `crates/holon-logseq-db/tests/fixtures/logseq-db/holontest.sqlite`
TWICE, in one process, produced two different serializations. The writer was
not at fault; the base is not byte-stable against itself.

Measured divergence at byte 7135, on block `00000002-1165-1650-8700-…`'s
neighbour:

```
A: "_logseq_raw/logseq.property/icon":{":type":":tabler-icon",":id":"list"}
B: "_logseq_raw/logseq.property/icon":{":id":"list",":type":":tabler-icon"}
```

Found outside any automated test, by a new spike assertion rather than by a
failing existing one — the existing suite was and remains fully green.

## Root cause
`holon_api::Value::Object` wraps `HashMap<String, Value>`
(`crates/holon-pattern/src/value.rs:35`). Rust's default hasher is seeded
randomly per process, so a `HashMap`'s iteration — and therefore serde's
emission — order is not a function of the content.

`project.rs:250-255` (`node_value`) carries a nested Transit map into exactly
that type:

```rust
TransitNode::Map(pairs) => Value::Object(
    pairs.iter().map(|(k, v)| (map_key(k), node_value(v))).collect(),
),
```

`TransitNode::Map` is an ordered `Vec<(TransitNode, TransitNode)>`, so the
authored order survives decoding intact and is discarded here, at the last
step. Any block with a `_logseq_raw/*` property holding a map of 2+ keys is
affected; the fixture has several (`logseq.property/icon` among them).

TWO IN-TREE CLAIMS ASSERT THE OPPOSITE and are false as written — a fixer
should delete or correct both rather than leave them contradicting the code:

- `crates/holon-logseq-db/src/base.rs:88-89` — "Ordered so a serialized base
  is byte-stable across runs and a diff of two base files is readable." The
  `BTreeMap` keyed by uuid does order the OUTER map; it says nothing about
  values, which is where the instability lives.
- `crates/holon-logseq-db/Cargo.toml` — the `serde_json` `preserve_order`
  feature is justified as "nested EDN carried opaque in `_logseq_raw/*` must
  round-trip in author order". That feature never applies: the value is never
  a `serde_json::Value` on this path, it is a `holon_api::Value`.

## Missing piece
No invariant compares SERIALIZED bytes. The interaction was already generated
and already asserted on: Inc 1's done-criterion
`the_fixture_round_trips_and_a_perturbation_is_seen_then_absorbed`
(`crates/holon-logseq-db/tests/import_base.rs:301`) imports the same fixture
twice and requires the diff to be empty — but `diff_against` compares
`BaseBlock`s with `==`, and `HashMap`'s `PartialEq` is order-insensitive. So
the equality oracle is blind to precisely the property that is broken, and
stays green over it. That is an ORACLE gap, not a COVERAGE one: the triggering
interaction is exercised on every run.

Note that the test was written specifically to avoid being vacuous (its
docstring rejects a plain "import twice reports nothing" because a diff that
always returns nothing would satisfy it). It closed that hole and still missed
this one — the perturbation it injects is a `content` string, which the
equality oracle does see. Byte-order was never the axis under test.

The keystone `general_e2e_composed_pbt.rs` cannot reproduce this — it does not
drive the LogSeq-DB importer at all — so no keystone red is available or
expected here.

## Remedy
OPEN. Not fixed in the W0 lane: the fix touches `holon_api::Value`, which is
flutter_rust_bridge-shaped and used across the tree, so the choice between
ordering it (`IndexMap`/`BTreeMap`) and narrowing the import path to a
serde_json value is an architecture call for the orchestrator, not a spike
decision.

Interim, in the W0 lane: leg 3 of `tests/kvs_round_trip.rs` compares a
CANONICAL serialization (recursively key-sorted) rather than raw bytes, and
says so in a comment. That is a real assertion about content, and it weakens
nothing about the round-trip — a re-encode that reordered a map's keys is
still caught by `every_row_re_encodes_to_the_value_it_decoded_from` (whose
`TransitNode` maps are ordered `Vec`s) and by leg 2's datom-level diff against
LogSeq's own `diff_graphs`.

Consequence to weigh when scheduling the fix: once W1 persists a base file
next to the graph, writing it twice from unchanged data produces a different
file, so every push would show a spurious VCS diff.
