---
id: 2026-07-27-atomic-rename-port-pairing-fallback-enters
date: 2026-07-27
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  Atomic-rename port's pairing fallback re-enters the cascade-delete path:
  `RenamePairing::classify` ran on EVERY notify event BEFORE org-relevance
  filtering and flushed a pending `From` as `(from, Remove)` on ANY
  interposing event (editor lock/sync-daemon write between the rename halves);
  the `Remove` routes to `on_file_deleted` where the D3 guard cannot fire
  (title not yet followed) → live doc cascade-deleted, recovery via re-ingest
  untested. Found by an adversarial VERIFIER code-reading the watcher layer —
  no automated test could reach it: the keystone enters below `NotifyWatcher`
  (`InMemoryFileSystem`), so `RenamePairing` was structurally untraversed.
source_line: 791
---

## Bug

Atomic-rename port's pairing fallback re-enters the cascade-delete path:
`RenamePairing::classify` ran on EVERY notify event BEFORE org-relevance
filtering and flushed a pending `From` as `(from, Remove)` on ANY
interposing event (editor lock/sync-daemon write between the rename halves);
the `Remove` routes to `on_file_deleted` where the D3 guard cannot fire
(title not yet followed) → live doc cascade-deleted, recovery via re-ingest
untested. Found by an adversarial VERIFIER code-reading the watcher layer —
no automated test could reach it: the keystone enters below `NotifyWatcher`
(`InMemoryFileSystem`), so `RenamePairing` was structurally untraversed.

## Root cause

atomic-rename port's pairing FALLBACK re-enters the cascade-delete path —
`RenamePairing::classify` ran on EVERY notify event BEFORE org-relevance
filtering and flushed a pending `From` as `(from, Remove)` on ANY
interposing event (editor lock / sync-daemon write between the two rename
halves); the `Remove` routes to `on_file_deleted` where the title-based D3
guard cannot fire (title not yet followed) → a LIVE doc is cascade-deleted,
recovery via re-ingest untested. Found by an adversarial VERIFIER
code-reading the watcher layer — NO automated test could reach it: the
keystone enters BELOW `NotifyWatcher` (`InMemoryFileSystem`), so
`RenamePairing` + the bridge's kind→`FileEvent` routing were structurally
untraversed. ENVIRONMENT primary. FIXED same commit: org-relevance filter
BEFORE pairing, timeout-only flush, id-based reunification before cascade;
red-first via a new notify-shaped ENVIRONMENT-parity rung driving synthetic
notify signals through the real `RenamePairing` → real bridge routing → the
controller.)

## Missing piece

A notify-shaped rung: synthetic notify events driven through the real
`RenamePairing` → `FileEvent` routing → `FileSyncController` (the pairing
unit tests stopped at `classify`; nothing integrated the flush path with the
controller). Full composed-keystone integration of a notify-shaped source
remains open parity work.

## Remedy

FIXED alongside (this commit): org-relevance filter BEFORE pairing,
timeout-only flush, id-reunification before cascade; red-first via the new
notify-shaped controller rung.
