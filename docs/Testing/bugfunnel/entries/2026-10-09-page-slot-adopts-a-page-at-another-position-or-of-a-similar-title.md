---
id: 2026-10-09-page-slot-adopts-a-page-at-another-position-or-of-a-similar-title
date: 2026-10-09
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  holon_api::page_slot bound a new page silently to an existing page under
  another parent, or to a page whose title only normalizes equal ("My  Notes"
  for "My Notes").
---

## Bug
Found by the recreate-lane round-3 verifier with two probes
(lane-logs/recreate3-verify/probe.log): `get_or_create_by_name_chain(["doc_904"])`,
asking for a ROOT page, returned the page `doc_904` that sits under
`block:61133fe7`; and a page titled `My  Notes` holding
`PageId::for_path("My Notes")` was adopted for `My Notes`. Nothing was created
and nothing was logged.

## Root cause
`page_slot` (crates/holon-api/src/identity_recognition.rs) answered `Existing`
from `recognize_derived_id`, which compares titles under `normalize_for_hash`
and ignores the parent, while the by-name lookups that run first
(`find_by_parent_and_name` in crates/holon-app/src/turso_seams.rs and
crates/holon-filesystem/src/sync_ports.rs) match `(parent, title)` exactly. The
`Existing` arm logged nothing.

## Missing piece
No generated sequence moves a page and then creates a page at its old path, and
the keystone generators never produce titles that differ only in case or
spacing.

## Remedy
FIXED. `page_slot(path, parent, title, holder)` binds only to a holder of
exactly `title` under `parent` (`PageHolder { title, parent }`); every other
holder is passed along the beside chain, and both the passed holders and an
adoption are `warn!`ed. Red: lane-logs/recreate4/red-ab.log; green:
lane-logs/recreate4/green-ab.log; teeth (parent check dropped / normalized
comparison put back): lane-logs/recreate4/teeth-a.log, teeth-b.log. Model
property `page_slot_follows_the_id_chain_model` and the real-writer property
`create_page_from_link_mints_the_model_ids`
(crates/holon/tests/create_page_from_link.rs) hold `page_slot` and the writer to
the id-chain model.
