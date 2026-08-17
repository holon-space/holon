---
id: 2026-08-07-file-level-drawer-placed-before-silently
date: 2026-08-07
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  A file-level `:PROPERTIES:`/`:ID:`/`:END:` drawer placed before `#+TITLE:`
  is silently discarded, the page is given a freshly minted UUID instead, and
  the app then rewrites the file with the user's three lines deleted.
source_line: 1174
---

## Bug

(overnight dogfood-explorer, same session) **A file-level
`:PROPERTIES:`/`:ID:`/`:END:` drawer placed before `#+TITLE:` is silently
discarded, the page is given a freshly minted UUID instead, and the app then
rewrites the file with the user's three lines deleted.** Seeded `Deep.org`
opened with `:PROPERTIES:` / `:ID: deep-page` / `:END:`; the page landed in
SQL as `block:76788551-6adb-f240-135f-b4d401dc157f` and the file came back
as `#+ID: 76788551-…` with the drawer gone. Holon's documented file-level
form is `#+ID:` and `docs/Reference/ORG_SYNTAX.md` does not claim to support
the drawer form, so the finding is NOT the non-support — it is that an
unrecognised construct in a user-authored file is dropped and overwritten
with no warning, no banner and no log line, which is the "silently degrades
to look fine" outcome the error-handling philosophy ranks last. Any org
author using the standard Emacs file-level property drawer loses it on first
write-back.

## Root cause

overnight dogfood — a file-level `:PROPERTIES:`/`:ID:`/`:END:` drawer placed
before `#+TITLE:` (valid org for file-level properties) is SILENTLY
DISCARDED and the page is given a freshly minted UUID instead, and the app
then rewrites the file with the user's three lines deleted. Holon's own
documented form is `#+ID:` and the doc does not claim to support the drawer
form, so the finding is not the non-support — it is that an unrecognised
construct in a user's file is dropped and overwritten with NO warning, no
banner and no log line, which contradicts the fail-loud-never-fake rule. Any
org author who used the standard file-level drawer loses it on first
write-back)

## Missing piece

Ingest is specified to be strict and fail-loud, but the strictness is not
expressed as an assertion over UNRECOGNISED input: nothing tests what
happens to org constructs the parser does not model, so silent-drop and
correct-parse are indistinguishable to every existing test. Missing piece =
a round-trip invariant that any input construct which is not reproduced in
the rendered output must raise a disclosed degradation (the machinery
already exists — this is exactly what the org-render DEGRADED path does for
marks, and it worked correctly in this same session).

## Remedy

FIXED 2026-08-07 — RULING: **PRESERVE** (Martin), not drop-with-warning: the
drawer round-trips and a file-level `:ID:` carries document identity in the
same role `#+ID:` does. MECHANISM: Holon now OWNS the file-level drawer
instead of asking orgize for it — `split_file_drawer`
(`crates/holon-org-format/src/parser.rs`) hand-reads it off the front of raw
content and returns the body with those lines removed, so orgize never sees
it; the cheap identity probe `FileFormat::doc_id_from_content`
(`parse_doc_id_any_carrier`) and the full parse call the SAME function
rather than two agreeing implementations; and `parse_org_file_with` carries
a divergence contract that REFUSES to parse if probe and parse ever
disagree, naming the file and both answers. Non-`:ID:` keys are preserved
verbatim in authored order on the doc-root (`file_properties`, a
`serde_json::Map`; `preserve_order` is now declared in holon-org-format's
own `Cargo.toml` so a hakari regen cannot silently alphabetize every drawer
in the vault). Two carriers that AGREE are both kept; two that DISAGREE are
rejected loudly instead of silently resolved. STORE SEAM — the defect the
first attempt missed: write-back renders the PERSISTED doc-root, not the
parsed one, so `OrgFormatAdapter::sync_document_metadata` now copies the
drawer onto it; without that the in-crate parse↔render tests stayed GREEN
while every real file still lost its drawer on first write-back, which is
why the acceptance test had to move to the seam. TWO LIVE COUNTEREXAMPLES
surfaced in verification, both the same class (probe/parse disagreement
resolving to a SILENT mint): an INDENTED ` :PROPERTIES:` (org's
`drawer_begin_node` opens with `space0`; the probe did not trim, so the
author's `:ID:` was rewritten in place with a minted uuid, zero log lines)
and a value-less `:KEY:` line (orgize's `node_property_node` requires
`space1`, so ONE such line voided the whole drawer — the file went
identity-less, `#+ID:` was stamped on, and the drawer sank below `#+TITLE:`
where org no longer reads it as file-level). Both fall out of the ownership
change rather than being patched individually; indentation canonicalizes to
column 0 and a value-less key re-emits as `:KEY: ` (that trailing space is
load-bearing — without it the line cannot re-parse and the drawer decays to
body text), both disclosed in `docs/Reference/ORG_SYNTAX.md` beside the
Emacs `%-10s` padding convergence, which is why byte-identity is claimed for
the canonical form and explicitly NOT for padded org-roam files. GAP CLOSED:
the ORACLE gap this row names — silent-drop and correct-parse being
indistinguishable to every existing test — is locked by
`crates/holon-orgmode/tests/file_level_drawer_seam.rs` (8 tests driving the
REAL `FileSyncController::on_file_changed` ingest→store→write-back path,
including both counterexamples and a never-seen-file case, since a
pre-seeded page resolves by name-chain and would hide a broken identity
probe) plus 12 unit cases in
`crates/holon-org-format/tests/file_level_property_drawer.rs`.
MUTATION-PROVEN, each reds exactly its own lock: A (probe ignores drawer)
reds the two identity tests; B (drawer copy removed from
`sync_document_metadata`) reds 4; C (drop `trim_start`) reds the indented
case; D (enforce orgize's `space1`) reds the value-less case; E (probe
blinded) reds 6 with the divergence error — the proof the divergence
contract is not decorative. LIVE: both counterexample files ingested by the
real GPUI app on a throwaway vault (port 8751, killed and deleted) kept
their authored ids and drawers, gained no `#+ID:`, logged no divergence, and
survived a subsequent live edit cycle. GATES: fmt clean; holon-org-format +
holon-orgmode + holon-filesystem 420/420; keystone-smoke 4/0 x2;
hand-authored 34/34 + 9/9.
