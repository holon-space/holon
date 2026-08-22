---
id: 2026-08-22-org-ingest-drops-collapsed-into-property-bag
date: 2026-08-22
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  An org file carrying `:COLLAPSED: t` ingests with the typed `collapsed`
  column left false and a stray uppercase `COLLAPSED` string in the untyped
  properties bag, so a user's fold state is silently lost on import.
---

## Bug

Ingesting an org file whose headline carries the fold marker:

```org
* Folded parent
:PROPERTIES:
:COLLAPSED: t
:ID: folded-parent
:END:
```

leaves the store disagreeing with the parser:

```
inv-blocks-match-ref/block_raw: block:folded-parent
  SUT:       properties: {"COLLAPSED": String("t")}, collapsed: false
  reference: properties: {},                        collapsed: true
```

`collapsed` is document state (Martin ruling 2026-07-11) — shared, synced,
survives restart — so this is a real data loss on import, not a view-state
nicety. Found by agent exploration (lane `gv-vocab`) while giving the new
`block "<id>" is collapsed` matcher a live home in the parity corpus.

Red log: `lane-logs/item3-green3.log` (the composed catalog divergence above).
Localization logs (lane `collapsed-bug`): `lane-logs/red-collapsed.log`,
`lane-logs/red-composed.log`, `lane-logs/red-composed-loro.log`.

A SECOND, independent drop sits underneath this one on the same path and is
recorded separately:
[2026-08-22-loro-create-projection-drops-fold-state](2026-08-22-loro-create-projection-drops-fold-state.md).
Fixing this entry's cause alone left the composed test still red at
`Integer(0)`, which is how that one surfaced. This entry is scoped to the ingest
side; that one to the Loro→SQL projection.

## Root cause

Two defects on the org-ingest leg, one per observed symptom.

**The false column — the ingest→ordering-authority seam drops the typed field.**
`FileSyncController::block_create_request`
(`holon-filesystem/src/file_sync_controller.rs`) packed a parsed block for the
ordering authority as a `BlockCreateRequest` of `{parent_id, id, content,
properties, edges}`. That struct had no slot for the typed `collapsed` /
`widget_only`, and the parser consumes `:COLLAPSED:` into `block.collapsed`
(`parser.rs:994`) rather than leaving it in `block.properties` — so the fold
state was dropped at the pack.

The correct `collapsed=Boolean(true)` param that `build_block_params` DOES emit
never rescues it: in `flush_pending_creates` those params are used only in the
`else` arm, when the authority did NOT persist the block. Under Loro block-CRUD
authority `create_in_tree_batch` returns `persisted = true` and the params are
discarded wholesale. Under the SqlOnly default `persisted = false` and the
params ARE used — which is why the SQL-only ingest leg is green on `collapsed`
and only a Loro-authority boot loses it. That asymmetry is the bug's whole
shape, and the discriminator that proves it: the same composed test with
`loro_enabled = false` PASSES the collapsed assertion.

`holon-logseq-db/src/ingest.rs::create_request` carried a byte-identical copy of
the same five-field construction, so a LogSeq-DB import lost fold state the same
way. Not filed separately: it is the same defect at a second call site, closed
by the same constructor, and it never escaped — it was found by reading during
this fix, not by anyone hitting it.

**The stray property — the ingest leg re-ingests its own writeback spelling.**
`Block::drawer_properties()` deliberately re-serializes `COLLAPSED` /
`WIDGET_ONLY` from the typed fields because org writeback needs them to recreate
the drawer; `build_block_params` iterates the same function on the INGEST leg,
where they are not properties at all.

REFUTED, recorded so it is not re-run: making `is_storage_column_key`
case-insensitive. It is case-sensitive on purpose (`:Sort_Key:` must stay an
ordinary property), and measurement showed it would not have restored the column
anyway — `crates/holon-app/tests/org_store_org_round_trip.rs` drives
parse → `build_block_params` → `SqlOperationProvider` → `CacheBlockReader` and
that leg writes `collapsed` CORRECTLY. So `value_to_sql` / `optional_bool` and
the org-writeback round trip — the two prime suspects the first localization
pass named — are both refuted.

## Missing piece

COVERAGE (primary). No transition sequence in the catalog could reach a
`:COLLAPSED:` ingest: nothing ever drove an org file carrying the marker into
the composed slice, and — compounding it — the one boot flag that decides
whether this bug is reachable at all, `loro_enabled`, was `false` in every
existing org-ingest gate, so even a scenario seeding `:COLLAPSED:` would have
passed on a Turso-only boot. The generator, not the assertion, was the binding
constraint: `inv-blocks-match-ref/*` compares `Block` field-by-field and DOES
cover `collapsed`, so an invariant WOULD have fired the moment a case reached
this state.

NOT dual — and an earlier revision of this entry claimed `secondary: ORACLE` on
reasoning that does not survive checking, so the retraction is recorded rather
than quietly dropped. That claim was that the stray property "reached that state
constantly and nothing flagged it". It cannot have: under the keystone's DEFAULT
Sql authority the stray `COLLAPSED` DOES reach `block_raw.properties`, and
`inv-blocks-match-ref` compares the properties map — so any case reaching it
would have gone RED, not passed unflagged. An assertion that would have fired is
not an oracle gap.

What is established: no catalog seed carries `:COLLAPSED:` (grepped), so the only
route to a folded block is a toggle at runtime — `ExpandToggle` / `ToggleCollapse`,
both routing through `RefToggleMut::set_expanded`, which mirrors the typed field
on the ref (`ref_caps/toggle.rs`). For the STRAY half to appear, that runtime
fold must then round-trip through an org FILE (write-back re-rendering
`:COLLAPSED:`, then a re-ingest re-parsing it), because `build_block_params` only
sees drawer keys on the ingest leg.

OPEN, and deliberately not asserted either way: whether the composed catalog ever
actually draws that write-back-then-re-ingest sequence on a folded block under Sql
authority. Both transitions exist in the headless alphabet and `SimulateRestart`
is exactly a touch-and-re-parse, so it looks reachable in principle — but whether
write-back fires for a fold-only change, and whether the pair is ever co-drawn,
was not measured. If it IS reachable, this was a latent red rather than a
coverage gap and the classification should be revisited; the new
`block_params` unit test now pins the behaviour regardless of which it turns out
to be.

## Remedy

FIXED in lane `collapsed-bug` — with a SCOPE LIMIT, stated first because the
headline above is only established for one of the two authorities:

**Loro-authority path FIXED (both drops). Separately, the log:4 parity scenario
(runtime `WriteOrgFile` under default Sql authority, in the COMPOSED harness)
still loses the fold; BOTH cold-boot and live-watcher ingest of the same fixture
are measured correct on both tables, so the loss is not in the ingest — OPEN, see
[2026-08-22-sql-authority-org-ingest-loses-fold-state](2026-08-22-sql-authority-org-ingest-loses-fold-state.md).**

* `BlockCreateRequest::of(block, parent_id)` (`holon-core/src/block_ordering.rs`)
  is now the ONE way an ingest packs a create intent, and it carries the typed
  fold scalars into the authority's property map — where `set_field("collapsed")`
  already writes them and `read_block_from_tree` lifts them back into the typed
  slots. Both duplicated construction sites (`file_sync_controller.rs`,
  `holon-logseq-db/src/ingest.rs`) now delegate to it, and the hand-rolled
  five-field constructions are deleted so the pattern cannot recur.
* `is_typed_field_drawer_key` (`holon-orgmode/src/block_params.rs`) refuses
  `COLLAPSED` / `WIDGET_ONLY` on the ingest leg — in the emit loop AND the
  previous-key removal loop. It is the exact sibling of the existing
  `is_edge_drawer_key` guard, which already refuses the drawer keys
  `drawer_properties()` reconstructs from typed EDGE fields (`REQUIRES`,
  `ADVICE_SUPPRESSED`, `contributes-to`); these two were the only non-edge typed
  fields lacking it. A narrow allowlist of the two keys Holon itself serializes,
  NOT a case-insensitive schema match.

Red → green evidence:

* `structural_pbt.rs::org_ingest_collapsed_marker_reaches_block_raw` — boots the
  composed SUT with `loro_enabled = true` from the `:COLLAPSED: t` file and reads
  `block_raw` directly. RED for the right reason before the fix:
  `an authored ':COLLAPSED: t' must reach block_raw.collapsed — got Integer(0)`.
* `org_store_org_round_trip.rs::collapsed_drawer_marker_survives_both_write_legs`
  — pins the OTHER leg (org-ingest and Loro param builders through the real
  store). RED before the fix on the stray property, and it is what refuted the
  false-column-in-SQL hypothesis.
* `block_params.rs::typed_field_drawer_keys_are_refused_but_case_variant_user_keys_survive`
  — pins the allowlist's narrowness: `:Sort_Key:` still survives as an ordinary
  property, which a case-insensitive match would have swallowed.

CAVEAT on what covers what, stated as a revert test so it cannot rot into a
vague claim: reverting `is_typed_field_drawer_key` alone leaves
`org_ingest_collapsed_marker_reaches_block_raw` GREEN (under Loro authority
`flush_pending_creates` discards the ingest param builder's output); the guard is
pinned ONLY by
`block_params::typed_field_drawer_keys_are_refused_but_case_variant_user_keys_survive`
and `org_store_org_round_trip::collapsed_drawer_marker_survives_both_write_legs`.
Of those two, only the `block_params` unit test is confined to the ingest leg;
`collapsed_drawer_marker_survives_both_write_legs` drives `block_to_params`
directly in its `Loro` arm and reds on drop 2 as well (measured). Deleting it
un-covers both halves at once.
* `logseq-parity/outliner.feature` — the scenario "A folded block carries its
  collapsed mark into the store" STAYS `@wip`, and not for this entry's bug.
  Un-`@wip`ed on the train it executes and REDS on the third drop above, under
  the shipped default Sql authority. Its comment now names that drop and its
  entry, so the tag is documented rather than silently re-parking a known loss.
  This entry's two drops are proven by the tests above, not by the corpus.
