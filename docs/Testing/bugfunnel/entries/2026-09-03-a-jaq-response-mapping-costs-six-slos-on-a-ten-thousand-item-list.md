---
id: 2026-09-03-a-jaq-response-mapping-costs-six-slos-on-a-ten-thousand-item-list
date: 2026-09-03
gap: ENVIRONMENT
secondary: null
status: NOTED
summary: >-
  The low-code mapping layer's response→rows step takes 1266 ms for a
  10 000-item list in a release build — six times the whole 200 ms
  interaction→projection budget. ADR 0034 names exactly this measurement as the
  mapping layer's kill criterion.
---

## Bug

Found while implementing Increment 4 of the low-code connections plan (lane
`lowcode-inc4`), by writing the measurement ADR 0034 and
`plan-lowcode-connections.md` §5 both name as the mapping layer's kill
criterion: response → rows over a 10 000-item list against the 200 ms p95
interaction→projection SLO.

Measured on an M-series Mac, `--release`, by
`crates/holon-kitchen/tests/shopping_mapping_cost.rs`, mapping a synthetic
10 000-item shopping-list response through the shipped
`assets/integrations/shopping.yaml` filter:

| Step | Measured | Note |
|---|---|---|
| compile the filter | 1 ms | paid once per connection, not per call — fine |
| `RowMapper::map_to_row_sets` | 1266 ms | **6.3× the whole 200 ms budget** |
| `CompleteSnapshot::from_rows` | 6 ms | negligible |

The cost is linear — roughly 0.13 ms per item — so it is not a pathological
case: a list of 200 items costs about 26 ms, which is fine, and the number
scales straight through the budget somewhere around 1 500 items.

Where it goes, measured by cutting the filter apart (release, same input):

| Variant | ms |
|---|---|
| the shipped filter, `map` only (no row-stream parse) | 726 |
| …with `group_by` removed | 642 |
| …with the `test("\\S")` non-empty check removed | 423 |
| validation only, no intermediate array | 6 |
| emit only, no validation | 138 |
| `map_to_row_sets` (adds the JSON-Lines serialize + parse) | 1266 |

So roughly: 540 ms in the JSON-Lines round trip through `parse_row_sets`,
300 ms in the per-field regex, 100 ms in `group_by`, and the rest in jaq's
per-value object construction. A `reduce`-based fold instead of `group_by`
measured 386 ms versus 726 ms, so about half the `map` cost is recoverable
without changing behaviour — it was NOT taken in this increment, because it
makes the filter markedly harder to read for a win that still leaves the
10 000-item case over budget.

## Missing piece

ENVIRONMENT: no automated test exercised a connector response at a realistic
upper bound before this one. The `rest` transport's existing mock tests use
handfuls of records, and the old bespoke Rust parse — which this mapping
replaces — was never measured at scale either, so there is no before/after
comparison: the entry records a property of the NEW mapping layer, not a
regression against the old parser.

## Remedy

**Ruled D92.a — ACCEPTED, no fix.** `NOTED` rather than `OPEN`: the measurement
stands on the record, and no work is pending against it.

The ruling reframes the budget: a full peer response is a poll or sync event,
not an interaction, and the mapping does not run on the interaction thread (it
runs inside `ShoppingOperations::execute_operation`,
`crates/holon-app/src/shopping_operations.rs:201`, an async operation on the
backend runtime — so a slow map shows up as a slow sync, never as a dropped
frame). The line to hold is what one interaction's DELTA costs. Revisit via
delta sync if a real peer ever exceeds ~1 500 items; ADR 0034's kill-criterion
text now says so.

The three options were, and remain if that day comes:

1. **Accept it.** No shopping list, calendar or task list Holon syncs today is
   near 1 500 items. The kill criterion was written for the general mapping
   layer, and the general case is not yet exercised by a real peer.
2. **Halve the `map` cost** by folding with `reduce` instead of `group_by` and
   by hoisting the non-empty check. Buys ~2×, costs filter readability, still
   over budget at 10 000.
3. **Attack the 540 ms round trip** in `RowMapper::map_to_row_sets`, which
   serializes every jaq output to JSON text and parses it back. That step
   exists so a mapping is held to exactly the rules a wasm plugin's stream is
   held to — one parser, not two. Reading `jaq_json::Val` straight into
   `TypedRowSet` would remove it and would also remove that guarantee.

Pinned by `crates/holon-kitchen/tests/shopping_mapping_cost.rs`, which prints
all three numbers and gates only an order-of-magnitude regression against this
measurement.
