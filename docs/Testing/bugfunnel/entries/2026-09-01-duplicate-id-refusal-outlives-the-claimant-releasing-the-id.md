---
id: 2026-09-01-duplicate-id-refusal-outlives-the-claimant-releasing-the-id
date: 2026-09-01
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  A file refused for a duplicate `#+ID:` is still refused after the claimant
  gives the id up by taking a fresh one — `doc_home` is never revised, and the
  claim check only stats the recorded path instead of re-reading its id.
---

## Bug

Found by the verifier probing the fix for
`2026-08-31-refused-duplicate-id-file-re-ingests-every-poll-forever`, by asking
what ELSE ends a duplicate-`#+ID:` refusal.

There are three ways out of a refusal, and the third one is silent:

1. the refused file is edited — handled (its `(mtime, size)` changes);
2. the claimant leaves disk — handled (the claim check re-stats it);
3. **the claimant stays on disk but stops claiming the id** — by being given a
   fresh `#+ID:`. This is the FIRST remedy `disclose_duplicate_doc_id` offers
   the user: *"Give this file a fresh `#+ID:`, or delete it if it is a stray
   copy."* Follow that advice on the winning file and the refused file is never
   ingested for the rest of the session, with nothing said about it.

Measured over six discovery ticks after the claimant took a new id: the refused
file's read count stays flat (`[2, 2, 2, 2, 2, 2]`) and it never adopts.

## Root cause

`crates/holon-filesystem/src/file_sync_controller.rs`. Two facts compose:

- `doc_home` is insert-only per id (`note_doc_home`, `:1882`). The only purge is
  `forget_file_state` (`:1889`), which retains by PATH — it fires when a file
  VANISHES, never when a file that is still there changes the id it carries. So
  the map keeps `old_id -> claimant path` after the claimant has moved on.
- `live_claimant_of` (`:1749`) resolves that stale entry and then only `stat`s
  the path. A successful stat is read as "still claimed". It never re-reads the
  claimant's `#+ID:` to confirm the claim it is enforcing still exists.

So the refusal is enforced against a claim that is no longer made.

## Missing piece

COVERAGE: no generator can reach the state. The keystone's org corpus mints
unique `#+ID:`s by construction, so a duplicate-`#+ID:` vault is ungeneratable
in the first place — and the transition this needs is narrower still: two files
sharing an id, THEN an edit that re-ids one of them.

ORACLE as the genuine secondary: even with that state generatable, no invariant
asserts the liveness property it violates — that a file refused for an id which
is no longer claimed eventually ingests. Every existing invariant judges a
state, and this defect is a state that never arrives.

**This entry is load-bearing for a reason beyond its own severity.** The bug is
pre-existing — at base the same refusal was equally permanent, it merely churned
while being wrong (a re-read storm that also never adopted). The RC-4 fix
removed the churn, which was the only outward symptom. So the soak guard
proposed in that entry — fail if one path is ingested more than N times without
changing on disk — can no longer catch this: the pathological case is now
silent and cheap. A guard for this class has to watch for a file that is
DISCLOSED as refused and never subsequently ingested, not for one that is
ingested too often.

## Remedy

Open. Fix direction: `live_claimant_of` should confirm the claim, not just the
claimant's existence — re-read the recorded home and check it still carries
`doc_id` before reporting a live claim. A home that no longer carries the id is
not a collision, exactly as a home that has vanished is not. That read is on the
refusal path (rare), not on the per-tick gate check, so it does not reintroduce
the storm; the gate would consult it only when its cheap stat says the claimant
is still there.

Note for whoever takes this: the same staleness makes `doc_home` answer with a
path for an id nothing on disk carries any more, which is worth checking against
the other `doc_home` readers rather than patching only the refusal path.
