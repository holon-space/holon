---
id: 2026-10-10-long-vault-file-name-fails-conflict-copy
date: 2026-10-10
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A vault file with a long name made every overruling change fail with "File name too long", because its conflict copy name exceeded the file-system limit.
---

## Bug
Found by the round-4a verifier of the `merge-resurrect` lane. The conflict copy
name is `<stem>.conflict-<UTC stamp>-<NNN>.<ext>`. For a vault file whose own
name was near the 255-byte limit, the copy name was longer than the limit, the
copy write failed, and the failure blocked the overruling write-back, so the
overrule could never be applied.

## Root cause
`holon_core::conflict_copy::path_for` appended the suffix to the whole stem
without a length bound (`crates/holon-core/src/conflict_copy.rs`).

## Missing piece
No test used a vault file name long enough for the suffix to cross the limit;
the keystone's file-name alphabet is short.

## Remedy
Fixed in 2127edd3: `MAX_NAME_BYTES` (200) bounds the copy name. A stem that is
too long keeps its start (cut at a char boundary) and gains `~<fnv1a of the
whole stem>` so two long names stay distinct. Pinned by
`a_long_name_is_shortened_to_a_distinct_copy_name` and
`a_long_file_name_still_gets_its_conflict_copy` (`crates/holon-app/tests`).
Open: a vault file name over about 236 bytes cannot be written back at all,
independent of conflict copies.
