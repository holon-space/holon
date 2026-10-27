---
id: 2026-10-27-a-crash-restore-loses-an-owed-outdent-when-the-row-vouches-an-unstored-base
date: 2026-10-27
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A crash while an Outdent's write-back is owed loses the Outdent at the next
  boot: the file row vouches for a merge base that was never stored, so the
  boot diffs the file against the store and takes the older indented file.
---

## Bug

A file `crw.org` holds two sibling blocks. The user indents `Call mom`, and
that write lands. The disk then refuses writes, the user outdents `Call mom`
again, and the process crashes with that write-back owed. After the boot
`Call mom` is still under `Buy milk`: the Outdent is lost. The boot also
raises `writeback-degraded`.

Found by the hand-authored keystone case
`crash-restore-a-file-equal-to-its-moved-merge-base-with-a-write-owed-is-disclosed-with-a-copy`
(crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl),
which a lane wrote for the crash-restore window. The generated keystone never
draws it.

## Root cause

Before each write-back, the write-back writes the file row with the empty
content hash and the hash of the merge base
(`crates/holon-filesystem/src/file_sync_controller.rs`, the
`write_file_row` call in the org write-back). That base hash was the hash of
`last_projection`, the bytes of the last landed write. Only the hash stamp
(`stamp_written_hashes`) stored those bytes in `.holon/bases/`, and the stamp
runs only when the loop is idle or at a clean stop.

Here the Indent landed and the Outdent's write-back started before a stamp.
So the row vouched for the indented bytes, while `.holon/bases/` still held
the ingested bytes. The crash skips the stamp. At boot `booted_base_for`
finds a base that does not match the row's hash, raises
`writeback-degraded`, and returns no base. The ingest then diffs the file
against the store: the indented file reads as an external Indent and
overwrites the store's Outdent.

## Missing piece

`CrashRestart` is hand-authored only (`weighted_generator` fails with
`HandAuthoredOnly`), so the generated keystone cannot reach a crash with a
write-back owed. A case for this sequence existed only after a lane wrote
it.
- COVERAGE litmus: no generated transition sequence reaches it; a
  hand-authored case was needed.
- ORACLE litmus: `inv-blocks-match-ref/org` and `inv-conditions-match-ref`
  went red as soon as a case reached the state.

## Remedy

The write-back stores the bytes of the last landed write as the merge base
before the row records their hash (`store_base_for`). The row then never
vouches for a base that is not on disk. The case keeps the Outdent and raises
no `writeback-degraded`. It still lacks the `crash-restore-ambiguous`
disclosure and its conflict copy, which the known-red row
`crash-restore-window-not-disclosed` covers.
