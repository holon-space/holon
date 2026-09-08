---
id: 2026-09-08-org-priority-ranks-c-first-and-the-drawer-spelling-stores-a-string
date: 2026-09-08
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  Org priority reaches the store in two shapes that both sort wrong — the `[#A]`
  cookie is stored as the integer 3 so `ORDER BY priority ASC` ranks C first, and
  the lowercase `:priority: A` drawer spelling overwrites that integer with the
  string "A", which SQLite sorts after every integer and which the renderer then
  drops from the file entirely.
---

## Bug

Found outside any test by the `now-query` lane while running the vault's
`Now.org` live query (report
`scratchpad/reports/lane-report-now-query.md` §Run 2, 2026-09-10). The query
sorts with `ORDER BY json_extract(b.properties,'$.priority') NULLS LAST` and
returned the least important work first, with a further group of rows sorted
after every numbered row.

Two measured facts, one per shape:

- Ingest maps the org priority cookie `[#A]`→3, `[#B]`→2, `[#C]`→1, so an
  ascending sort on the stored value is an ascending sort on *unimportance*.
- 41 blocks store `priority` as the STRING `"A"`/`"B"`. SQLite's type ordering
  puts every string after every integer, so those rows land in a block at the
  end regardless of letter.

Ruling D101.a (Martin, 2026-09-10) fixes this at the parse boundary: one
canonical stored representation that sorts A-first as an integer, and no `CASE`
expression in the query.

## Root cause

Two independent defects that happen to share the `priority` property key.

**The inversion.** `crates/holon-api/src/types.rs:670-675` declares

```rust
pub enum Priority { Low = 1, Medium = 2, High = 3 }
```

and `to_int`/`from_int` (`:678-689`) carry those discriminants straight into
storage: `crates/holon-org-format/src/models.rs:777-787`
(`OrgBlockExt::set_priority`) writes `Value::Integer(p.to_int())` into
`properties["priority"]`, and `crates/holon-orgmode/src/block_params.rs:115-117`
forwards the same integer as the ingest param. The enum's doc comment states the
intent — "Stored as integer in SQL (High=3, Medium=2, Low=1)" — so the ordering
is authored, not accidental; nothing anywhere states which direction a sort on
it is supposed to mean.

**The string.** The vault authors priority in the *drawer* under its lowercase
name (`/Users/martin/Workspaces/pkm/holon-pkm/Projects/Holon/Display Placement &
Resurfacing.org:61` — `:priority: A`), which collides with the internal property
key of the same name. `crates/holon-org-format/src/parser.rs:918-921` sets the
typed priority from the headline cookie first, and then the generic drawer loop
at `:959-1018` reaches its catch-all `else` arm —

```rust
block.set_property(key, holon_api::Value::String(value.to_string()));
```

— and **overwrites** the integer with the raw letter. `OrgBlockExt::priority()`
(`models.rs:770-775`) reads the property through `as_i64()`, so it now answers
`None`: the typed value is gone, not merely shadowed.

That has a second, unmeasured consequence on the write leg. With `priority()`
answering `None`, the renderer's cookie branch
(`crates/holon-org-format/src/models.rs:1347-1350`) emits no `[#A]`, and
`drawer_properties()` (`models.rs:857-880`) lists `priority` and `PRIORITY` in
`INTERNAL_KEYS`, so the authored `:priority: A` line is not re-emitted either.
Both carriers are dropped on write-back. The vault has blocks carrying BOTH
spellings on one headline (`** BLOCKED [#A] … :priority: A`), so this is
on-disk data loss, not only a sort defect. NOT yet reproduced against a build —
the lane's probe test never got a build slot (`lane-logs/probe-75200.log`, 0
bytes for ~45 min); it is read off the three cited functions and must be
confirmed before the entry flips to FIXED.

## Missing piece

**ORACLE (primary).** The interaction is fully generatable and no invariant
would have gone red. The keystone models `priority` as an opaque integer sort
property — `crates/holon-integration-tests/src/pbt/query_ast.rs:560` sorts on
it, `:667-714` seeds it with literal `Value::Integer(0/1/2)` — so the reference
model and the SUT agree on any monotone relabelling of the letters. Nothing
anywhere ties the stored integer back to the org letter it came from, which is
the only place the direction is observable.

**COVERAGE (secondary).** The string shape was not merely unasserted, it was
ungeneratable. The org round-trip PBT authors priority exclusively through the
headline cookie (`crates/holon-orgmode/tests/round_trip_pbt.rs:1100`,
`set_priority_on_headline`), and its drawer-property generator does not emit the
lowercase `priority` key, so no case ever produced the collision the vault
produces on nearly every headline.

## Remedy

FIXED in the `org-priority` lane (ruling D101.a).

`Priority` is now a newtype over the authored LETTER
(`crates/holon-api/src/types.rs`), stored as a RANK that ascends with
importance — `A` = 1 … `Z` = 26 — so `ORDER BY priority ASC` ranks A first with
no `CASE`, and `Ord` on the type IS that rank order, so sorting values and
sorting the stored integer cannot disagree. `Low`/`Medium`/`High` are gone: they
named an org cookie something it never was, and every call site had to be
revisited when the direction flipped (`to_int`/`from_int` → `rank`/`from_rank`
made the compiler enumerate them).

The string shape is gone at its source: the parser resolves the `:priority:`
drawer spelling into the typed field before the generic drawer loop can
overwrite it. Two carriers that disagree, and a drawer priority that is not a
letter, both refuse the parse loudly. A letter beyond A/B/C is now DATA (org's
range is configurable) rather than the panic it used to be.

**Migration.** Legacy inverted ints are not distinguishable from canonical ones
by value — both occupy 1..3 — so the migration re-derives rather than inspects:
`RENDERER_VERSION` (`crates/holon-filesystem/src/file_sync_controller.rs`) is
bumped `1` → `2`, which invalidates every persisted `file.content_hash` and
forces a one-shot re-parse of every org file through the corrected boundary.
Idempotent by construction — the re-ingest re-stamps each hash under the new
version, so only the first boot after the bump pays — and paired devices
converge under D68 because each re-derives from the same authored letters rather
than from a marker one side may already have consumed. This reuses the
documented facility rather than adding a second migration marker beside it; the
`_meta._schema_version` gate originally scoped for this is not needed and was
not built.

Covering tests (red first in `lane-logs/red3-orgformat-68097.log`, green in
`lane-logs/green1-58233.log` / `lane-logs/app1-82641.log`):

- `holon-org-format parser::tests::the_stored_priority_rank_ascends_with_importance`
  — red as `left: 3, right: 1`, the inversion itself.
- `holon-org-format parser::tests::the_drawer_spelling_parses_to_the_same_typed_priority_as_the_cookie`
  — red as `left: None`, the destroyed typed value.
- `holon-org-format parser::tests::a_priority_letter_beyond_the_default_range_is_data_not_a_panic`
  — red as the `[#D]` panic at `parser.rs:817`.
- `holon-org-format parser::tests::a_non_letter_drawer_priority_is_refused_loudly`
  and `…::a_cookie_and_drawer_that_disagree_refuse_the_parse` — the two loud
  refusals.
- `holon-app::org_store_org_round_trip::every_authored_carrier_stores_the_same_canonical_rank`
  — the value that actually lands in the store, for all three authored carriers
  on both production write legs.

A second consumer of the rank carried the inversion in PRODUCTION and was found
only in rev 2: the default WSJF `priority_weight` prototype
(`crates/holon-petri/src/lib.rs`) mapped rank 3 to the highest weight, so with
the rank corrected `[#C]` would have outranked `[#A]` in every task ordering.
Its own pinning test asserted that ordering, so it pinned the inversion rather
than catching it. Both are corrected, and the same inverted mapping was removed
from the docs and expression fixtures that teach it (`docs/Vision/PetriNet.md`,
`wiki/concepts/petri-net-wsjf.md`, `ARCHITECTURE.md`,
`crates/holon-api/src/computation.rs`, `…/expr_parser.rs`,
`…/tests/derived_field_dual_eval_pbt.rs`).

Covering test (red in `lane-logs/rev2-RED-d.log` — "priority 1 (`[#A]`) must
rank first" — green in `lane-logs/rev2-GREEN-final2.log`):
`holon-petri::tests::rank_output_is_pinned_for_priority_ordering`.

The write-back data loss noted above is filed on its own:
`2026-09-08-a-drawer-authored-priority-is-erased-from-the-org-file-on-write-back`.
Two further defects in the same family are filed separately:
`2026-09-09-a-priority-removed-from-the-org-file-keeps-its-stale-rank-in-the-store`
and
`2026-09-09-the-task-priority-view-reads-an-uppercase-key-the-org-parser-never-writes`.
