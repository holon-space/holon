---
id: 2026-09-19-condition-row-id-forms-no-uri-loses-row-identity
date: 2026-09-19
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  The `conditions` row source minted its `id` column from the bus's mirror key,
  which joins subject and kind with `\u{1F}` and so forms no URI, and after
  D142.a every degraded-state row lost its identity in the bounds registry.
---

## Bug

Found by lane `fix-gpui-bounds-entity-id` while root-causing the 42
`holon-gpui` windowed reds at integration tip `496a42c7`. One of them,
`named_source_rows_windowed a_named_source_paints_one_row_per_item`, is a real
product defect rather than a stale fixture.

The two condition rows PAINT — the window dump shows `text#2` and `text#3` at
900×26 each — but neither carries a `displayed_text` or an `entity_id`, so the
test's assertion that some painted element shows `todoist` fails with an empty
`Tracked: []`.

A probe in the gpui `text` builder printed the row's classified identity:

```
PROBE-UNUSABLE text.rs raw="condition:todoist\u{1f}integration-connect-failed"
PROBE-UNUSABLE text.rs raw="condition:claude-history\u{1f}integration-connect-failed"
```

Practical blast radius: a user with a failing integration still SEES the rows,
so this is not a blank-screen defect. What is lost is the row's addressability
— nothing can find those rows by entity in `BoundsRegistry`, so the MCP driver
cannot click or scroll to them and the `inv-displayed-text` oracle cannot read
what they paint.

## Root cause

`ConditionKey::mirror_key` (`crates/holon-api/src/condition_bus.rs:391`) joins
subject and kind with a `\u{1F}` unit separator, deliberately: the key order
groups a subject's conditions so the row source emits them contiguously.
`condition_row` then reused that key verbatim as the row's `id` column,
`format!("condition:{key}")`.

Before D142.a the `id` column was passed around as raw text, so the control
character was inert. D142.a made the column a parsed value: `row_id_of` sends
it through `EntityUri::try_from_raw`, the `\u{1F}` makes `condition:…` fail to
parse, the fallback `block:condition:…` fails too, and the column classifies
as `RowId::Unusable`. Every consumer that asks for the row's entity — the gpui
`text` builder's bounds tracking among them — then answers `None` and the row
goes untracked.

So the producer and the classifier disagreed about what the `id` column is
for. The column names an entity; a Holon-minted key that forms no URI is the
producer's bug, not the vault's. Nothing in the conditions path was
vault-authored, so D142.a's "a fault the vault authored" disclosure was never
the right answer here.

## Missing piece

**COVERAGE — gate scope, not suite content.** The oracle existed and had
teeth: `named_source_rows_windowed` asserts exactly this and went red the
moment the change landed. It simply was never run. The row-id lane's gate ran
only `--test row_id_boundary_windowed --test render_spec_row_id_windowed
--test live_block_id_windowed`, so a change to the shared classifier was
gated by three of the `holon-gpui` suite's binaries and the other reds
surfaced only in the later land gate.

Recorded as an escape because a production defect reached the integration tip,
but note for the ledger's reader: this is a gate-scope escape, not a hole in
the test suite. The remedy is a gate rule, not a new invariant — a lane that
touches a shared boundary (`row_id_of`, `EntityUri`, the render binding) runs
the whole `-p holon-gpui --features pbt` suite, not the binaries it happens to
have added.

No `id` column producer was checked against the classifier either. `row_id_of`
is now the contract for that column, and the conditions source was the one
producer that violated it; a sweep found no other.

## Remedy

FIXED in lane `fix-gpui-bounds-entity-id`.

`EntityUri::condition(subject, kind)` (`crates/holon-api/src/entity_uri.rs`)
mints the row's id, percent-encoding both parts exactly as `EntityUri::file`
encodes a path — the subject is an integration name from config, so URI-safety
is the constructor's to guarantee, not the caller's. `condition_row` builds the
id from the condition's own two parts instead of reusing the mirror key, which
keeps its `\u{1F}` and its grouping rationale untouched.

```
condition:todoist:integration-connect-failed
```

Red-first evidence: `named_source_rows_windowed` was red at the tip and is
green after (`lane-logs/gpui-baseline-tip-full.log` →
`lane-logs/gpui-after-fix-2.log`). Two direct producer-contract locks were
added in `condition_source.rs`'s test module and proved by inversion —
restoring the old `format!("condition:{}\u{1F}{}")` turns both red
(`lane-logs/api-lock-inverted.log`), and the fix restored byte-for-byte
(sha256 `73007aa4…`) turns them green again (`lane-logs/api-lock.log`).

## Relation to other entries

Same D142.a chain as
`2026-09-18-render-spec-ids-panic-the-frontend-render`, which handled the
case where a VAULT-authored id forms no URI. This entry is its mirror image: a
HOLON-authored id that forms no URI, where the right answer is to fix the
producer rather than disclose a fault.

The other 31 reds this lane fixed were not product defects — see
`2026-09-19-gpui-fixtures-assert-pre-d142a-raw-row-ids`.
