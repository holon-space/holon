---
id: 2026-08-31-refused-duplicate-id-file-re-ingests-every-poll-forever
date: 2026-08-31
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  A file refused for a duplicate `#+ID:` is never recorded as handled, so the
  new-file poller re-reads and re-parses it on every cycle forever — 61,654
  ingest attempts and 58 MB of log in one session.
---

## Bug

Found while diagnosing Martin's 2026-08-31 session log. The log is 58 MB /
122,343 lines, and **30,827 of them (25%) are one message about one file**:

```
DEBUG [FileSyncController] duplicate `#+ID:` still refused (already disclosed
  once at ERROR) doc_id=block:2905... refused=<one vault file>
```

First at 02:20:22, last at 20:26:15 — 18 hours, roughly one every 2 seconds,
never stopping. Counting the enclosing spans instead of the log line:

```
$ grep -o 'org.poll_new_files:org.ingest_file{path=[^}]*}' plain.log \
    | sed 's/.*path=//' | sort | uniq -c
  61654 <the refused file>
```

**That is the only path `poll_new_files` ever re-ingested.** Every other vault
file was ingested once and then left alone. This single signature is what made
the log look like a flood of errors; it is unrelated to the `ClaudeCode` page
that prompted the investigation.

The underlying vault condition is genuine — two org files under
`holon-pkm/Agents/` carry the same `#+ID:`, one being a derived copy of the
other — and refusing the second is correct behaviour. The defect is the retry
loop, not the refusal.

## Root cause

`crates/holon-filesystem/src/file_sync_controller.rs:1785-1812`. The duplicate
path has a **log** dedupe (`duplicate_id_disclosed` — ERROR once, DEBUG
thereafter) but no **work** dedupe: refusing the file returns without adding it
to the set of files the controller considers handled. `poll_new_files` still
sees an untracked file on disk on the next tick, reads it, parses it, resolves
the `#+ID:`, hits the same claimant and refuses again.

The log dedupe makes the cost invisible while leaving it in place. Per poll
this is a full file read plus an org parse of a file that cannot possibly
change outcome until it is edited.

## Missing piece

No test asserts that the second poll after a refusal does no work. The
keystone's org corpus generates unique `#+ID:`s by construction, so a
duplicate-ID vault state is not generatable at all — a COVERAGE gap in the
vault-state generator, not in the interaction alphabet.

There is also no standing guard on log volume or on repeated ingestion of one
path, which is why 61,654 redundant ingests ran for 18 hours unnoticed.

## Remedy

Open. Fix direction:

1. Record the refused canonical path in the controller's handled/known set
   (keyed with its mtime or content hash) so `poll_new_files` skips it until it
   actually changes. The existing `duplicate_id_disclosed` set already carries
   the right key — the refusal should suppress the *work*, and the log dedupe
   then follows for free.
2. Un-narrow the keystone's vault generator so two org files can share an
   `#+ID:`. The property to assert: after a refusal, a second poll over an
   unchanged tree performs zero ingest attempts. Red for the right reason
   before the fix.
3. Cheap standing guard worth considering separately: fail a soak if any single
   path is ingested more than N times without changing on disk.
