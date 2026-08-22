---
id: 2026-08-22-importbase-serialization-is-not-byte-stable
date: 2026-08-22
gap: ORACLE
secondary: null
status: FIXED
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
FIXED at the SERIALIZATION BOUNDARY, in W1 item 0.
`ImportBase::to_canonical_json` (`crates/holon-logseq-db/src/base.rs`) builds
the value and sorts every object's keys recursively (`sort_keys`, arrays keep
their order — a list's order is data, a map's is arbitrary), and `save` writes
exactly that. `tests/kvs_round_trip.rs` leg 3 now compares through the same
method rather than a duplicate local helper, so there is ONE canonicalizer.

DECISION FOR REVIEW (ratified by the orchestrator before implementation):
`holon_api::Value` was deliberately NOT changed. Ordering it (`IndexMap` or
`BTreeMap`) would be the more thorough fix, but `Value` is
flutter_rust_bridge-shaped (`crates/holon-pattern/src/value.rs:35`) and
reaches most of the tree, while only the PERSISTED form actually needs an
order. Accepted cost: in-memory `Value`s stay unordered, so nothing may assume
a nested map's key order survives a round trip through the base. Both the
method's doc comment and this entry say so.

Red → green evidence:
- Red `.lane-logs/w1-item0-red-final.log` — `two_imports_of_one_graph_persist_to_identical_bytes`
  FAILED, "the same graph persisted to different bytes, first differing at
  char 14814", the two windows showing `_logseq_raw/logseq.property/icon`
  emitting `:id`/`:type` in opposite orders. Red for the behaviour, not for a
  missing symbol: `to_canonical_json` existed as a non-canonical stub so the
  test compiled and failed on bytes.
- Green `.lane-logs/w1-item0-green.log` — full crate suite, 80 tests, 0 failed.

The regression pin is that test. It imports TWICE on purpose: serializing one
base twice cannot see the bug, because a `HashMap` iterates consistently
within its own lifetime.

The two false claims are gone: `base.rs`'s comment now says the `BTreeMap`
orders the outer map only and points at `to_canonical_json` for stability, and
`Cargo.toml`'s `preserve_order` rationale is rewritten to the true one (the
canonical form builds a `Value` in the order it wants, which a
`BTreeMap`-backed `Value` would re-sort).
