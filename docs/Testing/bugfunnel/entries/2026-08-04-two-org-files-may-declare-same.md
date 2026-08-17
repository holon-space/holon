---
id: 2026-08-04-two-org-files-may-declare-same
date: 2026-08-04
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  Two org files may declare the SAME `#+ID:` and Holon silently MERGES them
  into one page block instead of failing loud at ingest — with a data-loss
  consequence.
source_line: 777
---

## Bug

(dogfood on a COPY of Martin's real vault, port 8730, sandbox config+vault
under /tmp) **Two org files may declare the SAME `#+ID:` and Holon silently
MERGES them into one page block instead of failing loud at ingest — with a
data-loss consequence.** Observed on the real vault: `Projects/Agentic
DPL.org` and `Projects/DBG/Agentic DPL.org` BOTH begin `#+ID:
9464fbf0-412c-3aa7-f63b-8101088cc1c1` (verified by reading line 1 of each).
SQL therefore holds exactly ONE block named `Agentic DPL`
(`block:9464fbf0-…`), and the children of the two DIFFERENT directories
collapse under it: `SELECT id, content, parent_id FROM block WHERE content =
'Prototype'` returns TWO rows with the SAME `parent_id`, one per directory
(`block:d7999def-…` = the real `Projects/DBG/Agentic DPL/Prototype.org`, 2
children; `block:93c4e460-…` = `Projects/Agentic DPL/Prototype.org`, 0
children). The sidebar consequently paints two identical `Prototype` entries
under one `Agentic DPL` (screenshot `01-boot.png`). The DATA-LOSS
consequence is that write-back for the colliding document is then refused
for the rest of the session: the boot log carries `UNGROUNDED WRITE-BACK
REMOVAL: 31 of 31 on-disk block(s) would be DELETED … QUARANTINING this file
from write-back` for `Projects/DBG/Agentic DPL/Prototype.org` (fires once,
at 07:29:04.805184Z), i.e. every user edit to that page is silently never
persisted to disk from that moment on. PRECISION, corrected after
adversarial verification: the quarantine is **session-persistent, not
permanent** — the same log line ends "Un-quarantines on the next
fully-successful ingest", and it stayed in force here only because
`Prototype.org` was ingested exactly once (07:24:24, `done: 31 block(s)`)
and never re-ingested afterwards. Nothing in the evidence establishes
permanence; the user-facing substance — edits silently not reaching disk,
with no UI signal — is unaffected. The refusal itself is CORRECT and is
exactly ADR 0025 doing its job — the bug is one layer up: the duplicate
`#+ID:` was accepted in the first place, and the quarantine is disclosed
ONLY in the log, with no UI banner (see the companion `DegradedSignalBus`
row, which explains why no banner can appear).

## Missing piece

The test environment never contains two files with the same `#+ID:`, because
every fixture vault is generated with fresh ids — the ingest path's
behaviour on an id collision across files is simply not a state the harness
can reach. Missing piece is two things: (i) a fail-loud guard at the ingest
boundary that rejects (or quarantines, loudly and visibly) a second document
claiming an already-claimed `#+ID:`, per "parse, don't validate" — an id is
an identity, and two files claiming one identity is an illegal state that is
currently representable; (ii) a keystone/ingest fixture that seeds exactly
that collision so the guard has a red-first proof. Secondary COVERAGE
because the *interaction* (ingesting a vault containing the collision) is
also ungeneratable today.

## Remedy

OPEN 2026-08-04 — diagnosis only, no prod change in this lane. NOTE FOR
MARTIN: this is a PRE-EXISTING corruption in the real vault at
`/Users/martin/Workspaces/pkm/holon-pkm` (two files, one id) and is worth
repairing there independently of the code fix; until it is repaired, edits
to `Projects/DBG/Agentic DPL/Prototype.org` are not being written back.
