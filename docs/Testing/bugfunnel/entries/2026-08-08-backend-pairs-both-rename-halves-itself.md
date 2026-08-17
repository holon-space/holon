---
id: 2026-08-08-backend-pairs-both-rename-halves-itself
date: 2026-08-08
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  On any backend that pairs both rename halves itself (inotify/Linux), our own
  atomic write-back arrives as `RenameBoth { from: <temp>, to: page.org }` and
  was classified as a document RENAME
source_line: 759
---

## Bug

(task #24 lane, found by code exploration while proving the temp name
invisible to ingest — no test existed either way) **On any backend that
pairs both rename halves itself (inotify/Linux), our own atomic write-back
arrives as `RenameBoth { from: <temp>, to: page.org }` and was classified as
a document RENAME**, so `on_file_renamed` would re-home the page from its
own temp file.

## Root cause

task #24 lane, found by code exploration while proving the temp name
invisible to ingest — no test existed: **on a backend that pairs both rename
halves itself (inotify/Linux), our own atomic write-back arrives as
`RenameBoth{from: <temp>, to: page.org}` and was classified as a document
RENAME, so `on_file_renamed` would re-home the page from its own temp
file.** Latent on macOS only because FSEvents splits the halves.
ENVIRONMENT: the whole `RenameMode::Both` branch is reachable only on a
platform the suite never runs, and the macOS-only test environment cannot
produce the signal. Fixed with the same temp recognition; covered by a unit
test that constructs the `Both` signal directly. Evidence same file, §A/§C.)

## Missing piece

The `RenameMode::Both` branch is only reachable on a platform the suite
never runs; on macOS FSEvents splits the halves, so the macOS-only test
environment cannot produce the signal at all.

## Remedy

**FIXED in-lane 2026-08-08 (task #24)** with the same temp recognition, and
covered by a unit test that constructs the `Both` signal directly (the
branch previously had ZERO coverage — the verifier proved it by reverting it
with all tests still green). The non-temp `Both` behaviour is deliberately
left exactly as it was.
