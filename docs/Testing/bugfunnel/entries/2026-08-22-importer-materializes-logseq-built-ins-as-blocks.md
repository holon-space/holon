---
id: 2026-08-22-importer-materializes-logseq-built-ins-as-blocks
date: 2026-08-22
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  The LogSeq-DB importer materialized LogSeq's own built-in property, class and
  kv pages as user blocks, so 185 of the ImportBase's 206 blocks were content
  no person ever authored.
---

## Bug

Importing the committed LogSeq-DB fixture produced an `ImportBase` of 206
blocks. Only 21 of those entities are things a user wrote. The other 185 are
LogSeq's own furniture — its property definitions, its class pages, its
`:logseq.kv/*` records, its `logseq/config.edn` file entity — every one
surfaced to Holon as an ordinary block with a title and a parent.

Found by an agent census during W2 (the push increment), not by any test. The
census was written to answer a different question — "which blocks may `push`
legitimately target?" — and the 179/206 ratio fell out of it. Nothing was
looking for this.

The consequence is not cosmetic. Every Holon view of a LogSeq graph would show
LogSeq's schema as user content, every base file would be ~90% noise, and the
push layer had to grow a refusal specifically to stop Holon rewriting LogSeq's
schema pages — a guard whose entire reason for existing is this defect.

## Root cause

`crates/holon-logseq-db/src/project.rs` projects every entity carrying a
`:block/title` into a `Block`, with no notion that LogSeq marks some entities
as its own. `ImportBase::from_import`
(`crates/holon-logseq-db/src/base.rs:139`) then observes whatever the
projection produced. There is no filter at either point.

LogSeq's own predicate for this is `outliner-validate/built-in-entity?`, a
three-way OR — the `:logseq.property/built-in?` flag, OR a `:file/path`, OR an
internal `:db/ident`. Measured against the fixture it selects **192 of 213
entities** (`oracle/probe_built_in.cljs`; recorded in
`tests/fixtures/logseq-db/built-in-entities.json`). Holon implemented no part
of it until W2.

## Missing piece

**No invariant ever expressed what the base is FOR.** The importer's tests
exercised exactly this path on exactly this fixture — `tests/import_base.rs`
and `tests/holontest_import.rs` import the real graph through the real
importer, so the interaction was generated every run. What was absent is any
assertion of the form "the base holds only what a user authored".

Worse than absent: the existing pins actively BLESSED the defect. Several
tests asserted `base.len() == 206` as a fixed expectation, so the wrong number
was encoded as the correct one and any accidental fix would have failed the
suite. That is why this is an ORACLE escape and not a COVERAGE one — the
generator reached the state on every single run, and the oracle said it was
fine.

The lesson generalises past this bug: **a pin that records "what the code
currently does" without stating why that is right converts a defect into a
requirement.** Every one of the `206` assertions was written in good faith by
someone reading the number off a passing run — which is exactly how the base's
composition stopped being a question anybody asked.

## Keystone repro

Not reproducible in `general_e2e_composed_pbt.rs`: the composed keystone does
not import LogSeq-DB graphs at all, so no transition sequence reaches this
code. The crate-local equivalent is the census test in
`crates/holon-logseq-db/tests/push.rs`
(`the_fixtures_built_in_share_is_pinned`), which is where the gap is being
closed.

## Remedy

Ruled LW-7.a (Martin): the importer EXCLUDES built-in entities. They are still
READ for schema knowledge — property-name resolution, class names — and never
materialized as blocks. Push's built-in refusal stays as a backstop.

The exclusion also had to define what becomes of a non-built-in child of an
excluded parent. Measured: the fixture's four such entities are LogSeq's own
per-view UI records on the hidden `$$$views` page (`:logseq.property.view/
feature-type :linked-references`), all childless, and no other non-built-in
entity in the graph has a built-in parent. Keeping them with a dangling parent
ref, or re-homing them to a synthetic root, both turned out to require
WEAKENING an existing fail-loud invariant — with the parent excluded the
projection refuses outright — a `DanglingReference` from one of the four
panels to page 188 (WHICH panel is reported depends on entity iteration
order and is not stable across runs; all four share the same parent) — so
those options were not merely worse, they were unavailable without relaxing a
guard. Excluding by containing PAGE was ruled instead; it leaves the invariant
intact and the exclusion count is disclosed in the import summary.

Gap-closing rung, red first and recorded before any fix
(`.lane-logs/wbi-red.log`): the census assertion inverted from "206 blocks" to
"no built-in entity may be materialized in the base", which fails today with
185 built-in uuids listed. The fix is in progress in the W-builtins increment;
this entry stays OPEN until the exclusion lands and every test carrying the
old numbers is re-pinned to measured values rather than guessed ones.
