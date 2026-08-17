---
id: 2026-07-10-assigns-constant-sort-key-sibling-key
date: 2026-07-10
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  `block.create` assigns constant sort_key `A0` → sibling key collisions and
  nondeterministic order until a later op forces a re-mint (seen: date-block
  vs manual block, c1 vs c2, all three sidebar pages; split rebalanced to
  7F80/80/8180)
source_line: 883
---

## Bug

`block.create` assigns constant sort_key `A0` → sibling key collisions and
nondeterministic order until a later op forces a re-mint (seen: date-block
vs manual block, c1 vs c2, all three sidebar pages; split rebalanced to
7F80/80/8180)

## Missing piece

keystone's create transition never asserts unique sibling keys; no invariant
"sibling sort_keys strictly ordered/unique"

## Remedy

FIXED (stream 2026-07-10): root cause = `SqlBlockOperations::create` never
routed through `BlockOrdering::place`/`OrderKeyMinting`, so a sort_key-less
create fell to the `block_raw` column default `'A0'`. Fix: mint via existing
`gen_key_between(last_sibling_key, None)` when caller omits sort_key AND
store is the SqlOnly order owner (same gate as `new_child_anchor`;
Loro/Upstream untouched — invariant 10); caller-supplied key wins. Test
`create_without_sort_key_mints_strictly_increasing_keys`. Keystone
sibling-uniqueness invariant still open
