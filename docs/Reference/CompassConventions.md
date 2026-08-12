# Compass Property Conventions

The Compass layer (docs/Vision/LongTerm.md §"The Compass Layer: Intention as
Data") stores intention as ordinary blocks. Five drawer keys carry the whole
schema; everything else is content.

## The key set

| Key | Value grammar | Example |
|---|---|---|
| `compass` | the item type: `mission`, `problem`, `goal`, `strategy`, `challenge`, `dimension-current`, `dimension-ideal` | `:compass: goal` |
| `contributes-to` | bare block ID, or `[[Page Name]]` when the target is a page, or the sentinel `none` | `:contributes-to: compass-m1` |
| `last-reviewed` | ISO date or ISO datetime | `:last-reviewed: 2026-08-11` |
| `provenance` | `explicit` \| `inferred` \| `deduced` | `:provenance: explicit` |
| `review-cadence` | ISO-8601 duration | `:review-cadence: P30D` |

`contributes-to` (formerly `leads-to`) encodes one edge of the spine
`task → strategy → goal → mission`. Problems point at the mission they serve,
challenges at the strategy that works on them, a dimension's current state at its
ideal state, and an ideal state at its mission.

## The rules

1. **Lowercase kebab-case.** Org's own typed keys (`ID`, `TODO`, `TAGS`,
   `SCHEDULED`, `REQUIRES`, `COLLAPSED`, `WIDGET_ONLY`) are UPPER-CASE and are
   reinterpreted or dropped at render. A lowercase Compass key can never collide
   with one.
2. **Never prefix a key with `_`.** `drawer_properties()` filters `_`-prefixed
   keys, so the value survives in the store but is erased from disk on the next
   write-back — the one silent, one-way loss.
3. **Never author an empty value.** A valueless drawer entry does not come back
   from the parse at all; the key vanishes. Use an explicit sentinel (`none`)
   instead.
4. **Author the drawer in ASCII-alphabetical order, after `:ID:`.** Authored
   drawer order is replayed from `_drawer_order`, which the `_`-prefix filter
   removes from the DRAWER only — the carrier persists in the stored properties
   bag on both production write legs, the Loro projection writer and the
   file-ingest param builder, and comes back on the store-origin render path
   (pinned by `crates/holon-app/tests/org_store_org_round_trip.rs`; strip the
   carrier by hand and the same renderer alphabetizes). Alphabetical authoring is
   therefore a determinism convention, not a loss mitigation: it makes the
   template identical to the renderer's carrier-less fallback, so documents
   stay byte-stable even where a carrier is absent (hand-authored files that
   never carried one). The sort is byte-wise, so uppercase keys sort first:
   `ID`, `TEMPLATE`, `TEMPLATE_VARS`, `compass`, `contributes-to`,
   `last-reviewed`, `provenance`, `review-cadence`.
5. **Values are opaque text.** Multiword values, ISO datetimes, org links, and
   org markup characters survive verbatim. No escaping is needed.

## Why

Every rule above is a measurement, not a preference. The probe that established
them — a byte-stability matrix over both the file→file and the store-origin
render path, with the mechanism and `file:line` behind every dropped key —
is at
`/Users/martin/Workspaces/pkm/holon/.claude/worktrees/agent-aa45e3b11c56c8764/lane-report-compass-probe.md`.
Its recommended key set is pinned by
`recommended_compass_key_set_is_byte_stable` in
`crates/holon-org-format/tests/compass_property_key_probe.rs`.

`contributes-to` was added to that probe and measured before the rename landed:
SURVIVES and idempotent in every form the schema uses — bare slug, `[[id:…][…]]`
link, `[[Page Name]]` link, and the `none` sentinel — on both the file→file and
the store-origin path, with the canonical alphabetical document byte-stable
across two write-back passes.

Two premises that probe RETIRED: general underscores in identifiers do NOT get
mangled (only the `_` PREFIX loses data), and `INTERNAL_KEYS` are dropped at
render, not at ingest.

The gap it disclosed — no end-to-end org → store → org round-trip test — is
discharged: `crates/holon-app/tests/org_store_org_round_trip.rs` drives a real
pair (parse → SqlOperationProvider → Turso → CacheBlockReader → OrgRenderer)
through both production write legs — the Loro projection writer and the
org-ingest param builder — and pins that authored drawer order survives. The
probe's original carrier-loss premise is RETIRED with it: the probe had
hand-built carrier-less blocks, a shape the store never produces.

## Relation to Vision §3.1

docs/Vision.md §3.1 names the freshness triple `provenance` / `last_updated` /
`review_cadence`. The Compass spelling is `last-reviewed` and `review-cadence`:
kebab-case per rule 1, and `last-reviewed` rather than `last_updated` because
the Watcher's question is when a human last CONFIRMED the item, not when bytes
last changed.

## Templates

`assets/default/Compass.org` ships one template per item type, plus the
life-dimension current/ideal pair. Each declares its slots in
`:TEMPLATE_VARS:`; `reviewed` has no default so instantiation fails loudly
rather than minting a stale review date.

## `contributes-to` vs `requires` (ruled 2026-08-11)

Two different modal strengths, two different homes — do not conflate:

- `contributes-to` (Compass drawer property): CONTRIBUTION. Doing X advances Y;
  neither necessary nor sufficient. Drives the agenda query ("what advances
  goal X" = reverse closure over `contributes-to`).
- `requires` (the EXISTING typed edge, `Block.requires`,
  `crates/holon-api/src/block.rs:312`, `block_requires` junction table):
  NECESSITY. Y requires X means missing X blocks Y. Drives
  scheduling/blocking queries. Compass items needing a hard dependency use
  this existing field — never a new drawer key for it.

Using `requires` for contribution hides optional work from the agenda; using
`contributes-to` for necessity fakes hard dependencies out of soft ones.

## The layered contribution model (ruled 2026-08-11, PN dimension)

`contributes-to` is ONE QUERY SURFACE fed by three sources, distinguished by the
existing provenance ladder:

1. `explicit` — authored drawer property (the org files here).
2. `inferred` — Guide/Integrator suggestions; soft edges are the only safe
   target for inference (an inferred hard blocker would be a guess with veto
   power).
3. `deduced` — derived from the Petri Net where token semantics exist:
   reachability from a task's done-transition along output arcs to the place a
   measurable goal is bound to (goal metric = place marking + threshold).
   Three grades: structural (arc path exists — graph closure), algebraic
   (arc-weight product → quantified contribution, e.g. €5000/month), behavioral
   (holon-engine simulation, needed when competing transitions drain the same
   place). Freshness metadata (`last-reviewed`, `review-cadence`) governs
   re-derivation.

The PN grounds the hard/soft distinction itself: INPUT arcs are `requires`
(transition cannot fire without the token — enabling condition, the existing
typed edge), OUTPUT-arc reachability is soft contribution. Two relations =
the two arc directions of a transition; derivation refines the soft edge
where nets are modeled, authored edges cover everything unquantified.
