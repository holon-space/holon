---
id: 2026-08-17-post-write-hook-vault-snapshot-contention
date: 2026-08-17
gap: ENVIRONMENT
status: NOTED
summary: >-
  The org-write post_write hook failed 17 times in one session from vault
  VCS snapshot contention, self-resolving every time.
---

## Bug

Found by log analysis of `/private/tmp/holon-cold.log` (2026-08-17, real
vault session). 17 occurrences starting line 6501, e.g.
`[FileSyncController] post_write hook failed (exit=exit status: 255) for
/Users/martin/Workspaces/pkm/holon-pkm/Journals/2026-08-10.org: Warning:
Refused to snapshot some files:` followed later by `Concurrent modification
detected, resolving automatically.` and `Rebased 1 descendant commits...`.

## Root cause

`FileSyncController::run_post_write_hook`
(`crates/holon-filesystem/src/file_sync_controller.rs:5698-5730`) is a
fire-and-forget `tokio::spawn` running Martin's configured `post_write_hook`
shell command (external to Holon — a vault VCS snapshot command, evidently
jj given the "Rebased 1 descendant commits" text) after every org write. A
failure logs a `warn!`, not an `error!`, and is never retried or surfaced —
by design, per the function's own doc comment ("fire-and-forget"). The
underlying cause is the hook's own tool racing itself: rapid successive org
writes (task-state toggles in a live session) each trigger a snapshot
attempt, and a fast-enough write cadence causes the vault's own VCS to see
concurrent modification and refuse one snapshot in favor of resolving via
rebase on the next.

## Missing piece

ENVIRONMENT: this is Martin's own external hook script racing its own tool
under his personal write cadence — not something the composed keystone's
generic corpus could reproduce (it doesn't know what `post_write_hook` is
configured to, and the keystone doesn't drive real wall-clock-paced rapid
writes against a live jj/git-backed vault). No invariant gap either: nothing
inside Holon's own state is at risk here — the hook is fire-and-forget by
design specifically so a failure here cannot corrupt or block the app.

## Remedy

NOT ACTIONABLE from Holon's side beyond what already exists (loud non-fatal
warn, no retry-storm, no blocking). Every occurrence in this log
self-resolved (the hook's own tool rebased past the contention). Recorded
because 17-in-one-session is a real rate worth having on file if it ever
needs revisiting — e.g. if the hook script itself should debounce/coalesce
rapid successive invocations rather than firing one process per write — but
that would be a change to Martin's own hook script, not to Holon.
