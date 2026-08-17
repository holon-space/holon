---
id: 2026-08-07-answered-empty-block-uri-mount-node
date: 2026-08-07
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  `find_mount_by_shared_tree_id` answered `Some((tid, ""))` — an EMPTY block
  URI — for a mount node whose `STABLE_ID` had not landed
source_line: 1178
---

## Bug

(inspection, during the task-#30 settled-read withholding-coverage
migration) **`find_mount_by_shared_tree_id` answered `Some((tid, ""))` — an
EMPTY block URI — for a mount node whose `STABLE_ID` had not landed**
(`crates/holon-loro/src/loro_share_backend.rs`,
`read_stable_id(..).map(block_uri_from_bare).unwrap_or_default()`). Its one
caller is `accept_shared_subtree`'s idempotent re-accept branch, which
returns that string as `mount_stable_id` and projects it as the mount's SQL
block id — a durable, unaddressable row for a block that DOES have an
identity, just not a readable one yet. The silent-degrade is the
`unwrap_or_default()`: the one state the reader must not answer with is the
one it answered with a plausible-looking empty string. Reachability is
narrow and stated as such — under the doc-boundary RwLock
`create_mount_node` and `set_stable_id` sit inside ONE guarded write, so a
concurrent reader cannot normally observe the gap; the reachable arrivals
are a torn/interrupted accept and an imported mount whose meta lacks the
key.

## Root cause

secondary ORACLE: found by INSPECTION during the task-#30 settled-read
migration, while enumerating the readers that classify Loro nodes outside
`settled_read.rs` — `find_mount_by_shared_tree_id`
(`crates/holon-loro/src/loro_share_backend.rs`) answered `Some((tid, ""))`
for a mount node whose `STABLE_ID` had not landed, because it collapsed the
missing id with `unwrap_or_default()`. Its only caller is
`accept_shared_subtree`'s idempotent re-accept branch, which writes that
string as the mount's SQL block id — a durable unaddressable row, not a
transient miss. GAP: no fixture can arrange the state at all. Every
share/accept test runs `create_mount_node` + `set_stable_id` back-to-back
inside one guarded write, so a mount existing WITHOUT a `STABLE_ID` is
unreachable through the existing arrangements, and the keystone drives no
share/accept transition; secondary ORACLE because nothing asserts a
projected mount row carries a non-empty block URI either, so the empty id
would have flowed into SQL unremarked. Reachability stated honestly as
narrow — the doc-boundary RwLock closes the concurrent-reader window,
leaving a torn/interrupted accept or an imported mount lacking the meta.
FIXED same day: the lookup returns `anyhow::Result<Option<(TreeID,
String)>>` and errs naming the node and the shared-tree id; locked by
`a_mount_without_a_stable_id_errs_instead_of_naming_an_empty_block`, which
builds the half-born mount directly and is mutation-proven against the
restored `unwrap_or_default()`)

## Missing piece

No fixture can arrange the triggering state: every share/accept test runs
`create_mount_node` + `set_stable_id` back-to-back in one guarded write, so
a mount that exists WITHOUT a `STABLE_ID` is unreachable through the
existing arrangements, and the keystone has no share/accept transitions at
all — the interaction is ungeneratable, not merely unasserted. Secondary
ORACLE because even had the state been reached, nothing asserts that a
projected mount row carries a non-empty block URI, so the empty id would
have flowed into SQL unremarked.

## Remedy

FIXED 2026-08-07 (task #30 lane, same worktree as the fix): the signature
became `anyhow::Result<Option<(TreeID, String)>>` and a mount with no
`STABLE_ID` is now an `Err` naming both the node and the shared-tree id; the
sole caller already sits inside a `with_write` returning `anyhow`, so the
`?` propagates with no signature churn upstream. GAP CLOSED by
`a_mount_without_a_stable_id_errs_instead_of_naming_an_empty_block`
(`loro_share_backend.rs` tests), which constructs the
previously-ungeneratable half-born mount directly — `create_mount_node` with
NO `set_stable_id` — and pins both arms: `Err` containing "has no STABLE_ID"
before the id lands, and the correct `(tid, "block:99999999-…")` after it
does. Mutation-proven: restoring `unwrap_or_default()` reds it with `a mount
with no STABLE_ID must not resolve to an empty block id: Some((TreeID { ..
}, ""))`. Not a keystone repro — the keystone drives no share/accept
transition, so it is pinned at the `holon-loro` seam instead.
