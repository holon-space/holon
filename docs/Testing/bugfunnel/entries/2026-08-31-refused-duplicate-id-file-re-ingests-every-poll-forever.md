---
id: 2026-08-31-refused-duplicate-id-file-re-ingests-every-poll-forever
date: 2026-08-31
gap: COVERAGE
secondary: null
status: FIXED
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

`crates/holon-filesystem/src/file_sync_controller.rs`, `disclose_duplicate_doc_id`
(line 1802 after the fix). The duplicate
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

The retry loop is fixed; `ingest_file` now reports what it did instead of
returning a bare `Ok(())` that a refusal and an ingest were indistinguishable
through. `IngestOutcome::RefusedWhileClaimed(EntityUri)`
(`crates/holon-filesystem/src/file_sync_controller.rs:261`) travels up through
`on_file_changed`, and `poll_new_files` records the refused path as an
`IngestSkip` (`:278`) in the existing `ingest_quarantine`.

The skip is keyed on BOTH of the refusal's preconditions, because the refusal is
not a property of the refused file's bytes:

- the file's `(mtime, size)` — the same signature the poller already stats every
  tick to decide freshness (`:5098`), so no second mtime cache; and
- the CLAIMANT's liveness. The id is only taken while another file still holds
  it, so before honoring a skip the poller re-asks `live_claimant_of` (`:1749`,
  via `claimant_still_holds` at `:1786`) — a map lookup plus one stat of the
  claimant, never a read of the refused file. `live_claimant_of` is built for
  exactly this: "a claim whose file has VANISHED is a move or a rename, not a
  collision".

That second key matters because `disclose_duplicate_doc_id` tells the user the
winner is whichever file the session ingested first and that the scan order is
arbitrary — which invites them to delete the stray copy that won. Keying on the
bytes alone made that remedy a silent dead end: nothing about the refused file
changes when its claimant is deleted, so it would stay gated for the rest of the
session with no further disclosure. Either trigger lifting the skip also re-arms
the once-per-file identity disclosure, so a still-broken file is reported loudly
again rather than at DEBUG.

Pinned at the **file-sync-controller rung** (`poll_new_files` driven directly
over `InMemoryFileSystem`), in
`crates/holon-orgmode/tests/poll_new_files_containment.rs` — the file that
already owns the discovery pump's storm-gate:

- `refused_duplicate_id_file_is_not_re_ingested_while_unchanged`
- `an_edited_still_duplicate_file_is_re_disclosed_then_gated_again`
- `refusal_lifts_when_the_claimant_leaves_disk`

All three count `read_to_string` per path through a filesystem decorator, so
they assert on the WORK, not on the log lines the old dedupe already suppressed
— and not on stored page titles, which this test's doc-manager stub does not
re-write on adoption. Red before the fix at 4 reads where 2 were expected (one
extra read plus parse per tick, forever), at one disclosure where two content
versions warranted two, and — for the claimant rung, against a build with the
liveness re-check stubbed to `true` — at reads frozen where a re-read and
adoption were required.

Disclosed limitation, inherited rather than introduced: an edit that preserves
BOTH the mtime and the byte size leaves a refused file gated until something
else moves. Ordinary edits cannot reach it (APFS mtimes here are nanosecond;
two writes 10 ms apart differ), but mtime-restoring writers can — `touch -t`,
`cp -p`, `rsync --times`, `tar -x`, a restore from backup. This is the poller's
existing freshness key, not a new one: `poll_external_changes` keys
`disk_signatures` on the identical `(modified, len)` expression (`:5020`), so
the same edit is already invisible for ordinary tracked files. What is new is
that a refused file's re-ingest now depends on that key, where before it was
re-attempted unconditionally every tick.

Still open, tracked separately from this escape:

1. Un-narrowing the KEYSTONE's vault generator so two org files can share an
   `#+ID:`. The controller-rung tests above cover the mechanism; the keystone
   still cannot generate a duplicate-`#+ID:` vault at all.
2. A standing soak guard: fail if any single path is ingested more than N times
   without changing on disk. That guard, not a per-bug test, is what would have
   caught this class in 18 hours instead of never.
3. The sibling storm one branch away: the byte-syncer conflict-artifact skip
   (`file_sync_controller.rs:2542`) logs an unconditional ERROR and returns
   `IngestOutcome::Ingested`, so it is re-read and re-logged on every tick just
   as this one was — and it also counts itself into `poll_new_files`' return
   value every tick (measured: 10 reads, 10 ERRORs, `ingested`=10 over 10 ticks
   for one artifact), a false progress signal for anything reading that count as
   convergence. It is content/path-caused and permanent, so it belongs on the
   same gate — left out here only to keep this fix to one behaviour change.
