---
id: 2026-09-11-duplicate-doc-id-refused-without-banner
date: 2026-09-11
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A vault file refused for a duplicate document `#+ID:` was dropped silently —
  only its sibling refusal for a duplicate block `:ID:` raised the degraded
  banner, so the user lost a whole page with no disclosure.
---

## Bug
Martin's 10-hour production app log (one boot) shows three refused vault files.
Two were refused for a duplicate block `:ID:` and each raised a degraded-file
banner. The third, `Agents/citrix/citrix-STX.org`, was refused for a duplicate
document `#+ID:`:

```
ERROR DUPLICATE DOCUMENT ID … doc_id=block:2905bbe4-c5bc-736d-2b7b-87761c174714
```

because `Agents/citrix/citrix-STX.BROWSER_AGENT.org` carries the same `#+ID:`.
That refusal raised NO banner. The page was absent from the app and nothing in
the UI said so — a silent degrade, which the error-handling philosophy ranks as
the one outcome never to ship. Found by log triage of a production session.

## Root cause
The two refusals are siblings in `crates/holon-filesystem/src/file_sync_controller.rs`
and both refuse the WHOLE second file (D102.a), but only one disclosed:

- `disclose_duplicate_block_slug` built a `detail` string, logged it at ERROR
  and forwarded it to `WritebackDisclosure::ingest_refused`
  (`crates/holon-filesystem/src/sync_ports.rs:520`), which is the channel behind
  the degraded banner.
- `disclose_duplicate_doc_id` only called `tracing::error!`. The disclosure
  port was never reached, so no banner existed for the document-level case.

Both refusals cost the user exactly the same thing — one whole page — so the
asymmetry was not a deliberate policy, just an unfinished sibling.

## Missing piece
Two absences let it escape, one per gap:

- COVERAGE (primary). The composed keystone
  (`crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs`) cannot
  generate the triggering interaction. Its only raw-file-writing transition,
  `WriteOrgFile`
  (`crates/holon-integration-tests/src/pbt/transitions/write_org_file.rs:344`),
  stamps `#+ID:` from `state.doc_uri_by_name(doc_name)` — one minted URI per
  document NAME. Two files therefore always carry two different ids, and no
  transition writes a hand-chosen `#+ID:`. The precondition for the refusal is
  unsatisfiable by construction.
- ORACLE (secondary). Nothing in `crates/holon-integration-tests/` references
  `WritebackDisclosure` or `ingest_refused` at all, so even if a duplicate id
  were somehow generated, no invariant observes whether a refusal was
  disclosed. A refused-file-is-disclosed invariant does not exist.

Prod/keystone parity: closing the coverage half needs a transition that writes
an org file carrying an id the reference state already assigned to a DIFFERENT
document (the keystone knows both ids, so the reference can predict the
refusal), plus a composed-SUT seam that captures `ingest_refused` calls so an
invariant can assert "every refused file raised exactly one disclosure naming
both paths and the id". Both are keystone-catalog work well beyond this fix and
are left open; the bug itself is pinned by the dedicated test below, which is
the shape the invariant would later generalise.

## Remedy
`disclose_duplicate_doc_id` now builds its `detail` the same way its sibling
does and routes it through the shared `raise_ingest_refused_banner` helper, so
both duplicate-id refusals reach the banner with a reason naming both files and
the id. The shared banner leg (adapter-format lookup plus the
`ingest_refused` call) is factored into that one helper rather than duplicated.

Pinned by `a_second_file_claiming_a_doc_id_is_refused_whole_and_disclosed` in
`crates/holon-orgmode/tests/sync_controller_mutation_pbt.rs`, mirroring the
block-`:ID:` test beside it. Shown red for the right reason (zero disclosures
raised) before the fix and green after.
