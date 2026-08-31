---
id: 2026-09-01-turso-less-ingest-seam-stores-marks-as-a-property
date: 2026-09-01
gap: ORACLE
secondary: COVERAGE
status: OPEN
summary: >-
  On the Turso-less profile the org→block ingest seam never removes `marks`
  from the params bag, so a block's inline marks are stored as a property
  string and never become Peritext — silently, with no error.
---

## Bug

Found by code audit while fixing
[2026-08-31-marks-written-against-stale-content-quarantines-file](2026-08-31-marks-written-against-stale-content-quarantines-file.md)
(RC-3) — not by any automated test. Reading the two org→block write seams side
by side to check whether the RC-3 field-ordering hazard existed on both, the
Turso-less one turned out to have a different and worse defect: it does not
write marks at all.

Observable consequence on the Turso-less profile: a block whose org source
carries `[[wiki links]]` or `*bold*` ingests with its inline markup stripped to
the plain label in `content` and its `marks` nowhere the read boundary looks.
Marks are truth, not derived (ADR 0025 / the links ruling), so the next
write-back re-emits the block as plain text and the authored link syntax is
gone from disk. Nothing logs, nothing fails — it is exactly the "silently
degrades to look fine" case the repo's error-handling philosophy ranks last.

Not yet reproduced against a running Turso-less instance; the mechanism below
is read off the source, and the missing rung is precisely why no test says
otherwise.

## Root cause

`crates/holon-app/src/loro_seams.rs:438` — `LoroBlockOrdering::update_in_tree`,
"the whole org→block write path of a no-Turso session"
(`crates/holon-app/tests/loro_seam_edge_fields.rs:1`). It decomposes the params
bag by removing the fields it understands: `id`, the position hint, the routing
hint, `parent_id`, then `content` / `content_type` / `source_language` as one
typed `BlockContent` (`:457-472`), then every `EdgeField` to its junction
(`:476-491`). Whatever is left becomes properties (`:493`) and is written with
`update_block_properties` (`:540-543`).

`marks` is in that leftover set. `holon_orgmode::build_block_params` emits it as
a JSON string (`crates/holon-orgmode/src/block_params.rs:58-62`), so on this leg
it lands in the Loro meta property map under the key `"marks"` and never reaches
`update_block_marked` / the `LoroText` Peritext container at all.

Contrast the Turso/Upstream leg, which is correct: `SqlBlockOperations::
update_in_tree` routes each field through `set_field` →
`BlockCellRegistry::write_field`, whose `"marks"` arm
(`crates/holon-loro/src/block_cell_registry.rs:821-846`) calls
`update_block_marked`. That is the leg RC-3 was about, and it is why RC-3
presented as a loud quarantine while this one presents as nothing.

OQ-1 (`docs/Plans/BlockGeneralization.md:38`, ruled 2026-08-21) makes Turso-less
a **constrained profile** — but it constrains *reactivity* ("CRUD +
`computed_live` + simple filters; IVM-grade reactivity requires Turso"). Content
fidelity is not on that list, so losing a block's marks is not a licensed
degradation of the profile; it is a defect within it.

## Missing piece

**ORACLE (primary).** No invariant certifies that a block's marks survive
ingest, on any leg. The only mark invariant is
`inv-mark-bounds-within-content`
(`crates/holon-integration-tests/src/pbt/composed/invariants/mark_bounds_within_content.rs`),
which validates the spans of marks that are PRESENT — a block whose mark set was
silently dropped satisfies it vacuously. There is no `inv-marks-match-ref`, and
the reference comparison that would have caught it
(`inv-blocks-match-ref/loro`) does not carry marks. So even a case that hit this
state would have gone green.

**COVERAGE (secondary).** A Turso-less draw is reachable —
`set_for_wiring` (`crates/holon-integration-tests/src/pbt/composed/wide_e2e.rs:1387-1393`)
explicitly turns a no-Turso wiring into a Loro-only one, and calls the shipped
default `crdt.enabled = false` "a drawable mode". What is unverified is whether
the generator ever composes that draw WITH an org ingest carrying inline markup;
if it does not, generation never reaches the seam either. The existing
end-to-end mark test
(`crates/holon-integration-tests/tests/boot_suite/wiki_link_ingest_marks_junction.rs`)
covers only the two Turso-bearing modes — `.without_loro()` (SqlOnly) and the
Loro+Turso default. The Loro-without-Turso corner has no rung at all.

## Remedy

Open — deliberately not fixed in the RC-3 lane, which was scoped to the
quarantine mechanism on the Turso leg.

Fix direction:

1. Mirror the SQL leg's decomposition in `loro_seams.rs`: remove `marks` from
   the bag alongside `content`, parse it with `holon_api::marks_from_json`, and
   write the pair through `update_block_marked` so content and marks are one
   write. Reuse the RC-3 rule rather than re-deriving it — `content` must
   establish the text the spans address (see
   `ingest_field_write_order` in `crates/holon/src/core/sql_block_operations.rs`),
   and `update_block_marked`'s new per-call span precondition then makes any
   desync on this leg loud instead of silent.
2. Consider making the leftover bag closed rather than open on this seam: today
   any field the seam does not name degrades quietly into a property, so `marks`
   is the instance we found rather than the only one it can produce.
3. Close the ORACLE gap first, so the fix has a red to turn green: an invariant
   that a block's marks match the reference model's, wired wherever a
   `SutBackend` is present, plus a Loro-without-Turso rung for the wiki-link
   ingest test.
