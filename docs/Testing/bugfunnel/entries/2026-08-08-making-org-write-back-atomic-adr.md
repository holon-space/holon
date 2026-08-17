---
id: 2026-08-08-making-org-write-back-atomic-adr
date: 2026-08-08
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  Making org write-back atomic (ADR 0030 D3.1) turned every write-back into a
  rename, and `RenamePairing` let that rename's `To` half claim an unrelated
  pending `From`
source_line: 758
---

## Bug

(task #24 lane, found by adversarial verification driving the LIVE macOS
`NotifyWatcher`, after every in-repo gate had passed green) **Making org
write-back atomic (ADR 0030 D3.1) turned every write-back into a rename, and
`RenamePairing` let that rename's `To` half claim an unrelated pending
`From`**: a user moving `a.org` out of the vault followed by ANY write-back
emitted `Rename { from: a.org }` on `page.org`, which `on_file_renamed` acts
on by re-homing document A onto page.org's file and reconciling page.org's
bytes into it — the DOUBLE-HOMED class 090aac66 fixed — while the genuine
move-out's `Remove` was silently consumed. Write-backs are the most frequent
relevant signal in the system, so the exposure was every 500ms pairing
window after any move-out.

## Root cause

task #24 lane, found by adversarial verification on the LIVE macOS watcher
after every in-repo gate passed green: **making org write-back atomic turned
every write-back into a rename half, and the pairing state machine let that
half claim an unrelated pending rename `From` — so a user moving `a.org` out
of the vault followed by any write-back re-homed the moved-out document onto
`page.org` and silenced the move-out's `Remove`.** ENVIRONMENT: the
in-memory `FileSystem` double emits ONE synthetic `FileChange` and
structurally cannot produce the raw `RenameFrom`/`RenameTo` shape the real
adapter derives from a rename, and every org-side test plus keystone-smoke
drives the double — so 83 holon-filesystem + 160 holon-orgmode +
keystone-smoke were all green with the regression live. Secondary COVERAGE:
no test had ever driven `RenamePairing` with a pending `From` across a
write-back. Fixed in-lane; gap closed by a live-`NotifyWatcher` end-to-end
test plus unit tests built from the RECORDED live signal sequence, and the
double now emits the same shape the production pairing produces. Evidence
`docs/Testing/fixture-logs-2026-08-08/task24-atomic-write-watcher-hijack.txt`.)

## Missing piece

The in-memory `FileSystem` double emits ONE synthetic `FileChange` per write
and structurally cannot produce the `RenameFrom`/`RenameTo` pair the real
adapter derives from a rename; every org-side test and keystone-smoke drives
the double, so the entire suite is blind to signal-shape changes. 83
holon-filesystem + 160 holon-orgmode + keystone-smoke were green with the
regression live. Secondary COVERAGE: no test had ever driven `RenamePairing`
with a pending `From` across a write-back — the pairing's own unit tests
each start from an empty buffer.

## Remedy

**FIXED in-lane 2026-08-08 (task #24).** Root cause: our temp is
deliberately non-`.org` so ingest ignores it, and the pairing used the SAME
relevance predicate — so the temp's `From` half was discarded while the
target's `To` half stayed eligible to pair. Fix: a shared
`fs_port::atomic_temp_target` predicate (one place defines the convention,
`atomic_temp_path` mints it and the pairing reads it back); a temp `From`
now ARMS a self-replacement instead of being discarded, and the matching
`To` emits the plain `Create` an in-place write emitted, leaving the pending
`From` for its own `To`. A genuine relevant `From` clears the armed slots,
so `mv other.org page.org` still pairs. GAP CLOSED, not just the bug: a
live-`NotifyWatcher` end-to-end test now runs the exact user scenario, the
unit tests are built from the RECORDED live signal sequence (§A of the
evidence), and the in-memory double now emits the same `Create` shape the
production pairing produces. Red-for-the-right-reason for all four, incl.
the live one, in
`docs/Testing/fixture-logs-2026-08-08/task24-atomic-write-watcher-hijack.txt`
§C. Residual, disclosed: recognition depends on the temp's `From` half being
delivered (empirically it is, §A) and on it not being interleaved with a
genuine rename's `From`.
