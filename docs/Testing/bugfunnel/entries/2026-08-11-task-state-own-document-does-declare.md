---
id: 2026-08-11-task-state-own-document-does-declare
date: 2026-08-11
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A task state its own document does not declare grows one extra keyword in
  the block's title on every app restart, without bound.
source_line: 732
---

## Bug

(task #68 dogfood re-entry gate; found by DOGFOODING the live GPUI app; no
automated test produced it) **A task state its own document does not declare
grows one extra keyword in the block's title on every app restart, without
bound.** On a page declaring `#+TODO: NEXT WAITING \ | DONE`, a block put
into `TODO` is written back as `* TODO ordinary line!` with no declaration
check, and the next boot re-ingests the undeclared keyword as ordinary title
text. Measured over three cold boots on one vault: `ordinary line!` → `TODO
ordinary line!` → `TODO TODO ordinary line!`, with disk following each time.
The editor half of this class IS closed — `editor.source_projection` refuses
to project the same keyword and states the consequence — so the guard exists
on the surface the user types into and is absent from the surface that owns
the file. Not proven new: the write-back path is not code the rebuild
touched.

## Root cause

task #68 dogfood re-entry gate, found by DOGFOODING the live GPUI app: **a
task state its own document does not declare grows one extra keyword in the
block's title on EVERY app restart, without bound.** Page declares `#+TODO:
NEXT WAITING | DONE`; a block on it is put into `TODO` (the verifier's own
recipe, `set_field(task_state)`); the org write-back renders `* TODO
ordinary line!` with no declaration check, and the next boot re-ingests the
undeclared keyword as ordinary title text: boot 1 `ordinary line!` -> boot 2
`TODO ordinary line!` -> boot 3 `TODO TODO ordinary line!`, disk following
each time. Measured across three cold boots on one vault. The editor half of
this class IS closed — `editor.source_projection` refuses to project the
same keyword and says why ("…would read back as ordinary text and demote the
task") — so the refusal exists on the surface the user types into and is
absent from the surface that owns the file. COVERAGE primary: no draw sets
an undeclared task state and then restarts. ORACLE secondary:
`inv-org-render-fixed-point` is exactly the invariant that would convict,
and it is one of the 28 checks `run_self_checks` reports `skipped` against a
live app. Missing piece: a generator arm that writes a task state outside
the document's declared ring, plus a restart, with the render/re-ingest
fixed point asserted.)

## Missing piece

COVERAGE: no draw sets a task state outside the document's declared ring and
then restarts. ORACLE (secondary): `inv-org-render-fixed-point` is precisely
the invariant that convicts, and it is one of the 28 checks
`run_self_checks` reports `skipped` against a live app (`no live source for
SUT capability SutOrgRender`). Missing piece: a generator arm that writes an
undeclared task state, a restart, and the render/re-ingest fixed point
asserted across it.

## Remedy

CLOSED (task #100). GAP CLOSED FIRST:
`crates/holon-orgmode/tests/reingest_task_promotion_idempotent.rs::an_undeclared_task_state_is_a_cold_boot_fixed_point`
— three cold boots of render (the write-back renderer) → ingest (the real
`FileSyncController`), asserting the file is byte-stable AND the stored
content never absorbs a keyword. RED at `8570a14a`: content `ordinary line!`
→ `TODO ordinary line!` on the first boot. ATTRIBUTION confirmed
pre-existing: the unconditional keyword render is `render_headline_block`
(`crates/holon-org-format/src/models.rs`), whose `if let Some(ref todo) =
block.task_state()` block predates arm (d) — arm (d) touched no file in the
write-back render path. FIX at the seam the ledger names:
`OrgRenderer::render_document` resolves the document's own declaration
through `TaskKeywordVocabulary::from_declared(doc.todo_keywords())` — the
same chain the editor's surface uses — and `refuse_undeclared_task_state`
drops an undeclared keyword from the RENDER (WARN, block id + keyword +
declared set). Same refusal the editable surface already makes: a surface
that cannot SHOW the keyword must not write it. Disclosed cost: while the
keyword stays undeclared it is not durable on disk — strictly smaller than
the alternative, which loses it AND corrupts the title. Document-less render
paths (`render_entitys`, the dense projection) pass `None`: "no vocabulary
known" is not "declares nothing", the representability rule
`Surface::Pending` encodes.
