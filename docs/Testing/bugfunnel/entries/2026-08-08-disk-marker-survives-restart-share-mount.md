---
id: 2026-08-08-disk-marker-survives-restart-share-mount
date: 2026-08-08
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  An on-disk `share-role` marker survives a restart but the share-mount
  REGISTRATION does not, so edits to those subtrees are disclosed "org file
  pending" forever and never reach disk.
source_line: 1190
---

SUPERSEDED for the 4×-toast signature by
`2026-08-18-cold-boot-discloses-shared-edit-for-every-share`: the same banner
on a 2026-08-18 cold boot carried ZERO of the `share-role drawer property was
found…` WARNs this row's root cause rests on, and fired once per shared tree
in the vault rather than per lost registration.

## Bug

(Martin dogfooding his live instance; code-read + log-signature root cause,
NOT reproduced live) **An on-disk `share-role` marker survives a restart but
the share-mount REGISTRATION does not, so edits to those subtrees are
disclosed "org file pending" forever and never reach disk.** The 4x toast is
`DegradedKind::SharedSubtreeNotMaterialized` (`share_ui.rs:1631-1635`),
raised when a block carrying a `shared_tree_id` has an owning page that is
not a registered mount (`crates/holon-orgmode/src/di.rs:796-815`, from the
live-upsert path `:633-645` then `:823-832`). The corroborating log line
appears exactly 4 times on 4 different pages: `a share-role drawer property
was found on a page that is NOT a registered shared-subtree mount —
ingesting it as a normal file`; the block id Martin quoted appears once,
inside the `creates_ids` of the ORGSYNC_DIFF for one of those files. The 4x
is not a retry loop — disclosure is deduped per `shared_tree_id`
(`di.rs:816-822`) and the bus upserts by `(subject, kind)`. The edit is
durable in Loro and SQL and syncs to peers, but the disk projection never
catches up: no retry, no flush, and a repo-wide search found no site
clearing `SHARED_SUBTREE_NOT_MATERIALIZED`. Correctly disclosed, hence not
P1, but a real disk-durability hole. DISCLOSED CASUALTY: not reproduced live
— setting up a share and restarting to lose the registration was out of
scope; that reproduction is the work still owed.

## Root cause

Martin dogfooding his live instance — **an on-disk `share-role` marker
survives a restart but the share-mount REGISTRATION does not, so edits to
those subtrees are disclosed as "org file pending" forever and never reach
disk**. The toast Martin reported 4x is
`DegradedKind::SharedSubtreeNotMaterialized`
(`frontends/gpui/src/share_ui.rs:1631-1635`, detail = the block id at
`:308-315`), raised when a block carrying a `shared_tree_id` has an owning
page that is not a registered share mount
(`crates/holon-orgmode/src/di.rs:796-815`, reached from the live-upsert path
`:633-645` → `:823-832`). The corroborating log signature appears EXACTLY 4
times, on 4 DIFFERENT pages: `[FileSyncController] a share-role drawer
property was found on a page that is NOT a registered shared-subtree mount —
ingesting it as a normal file`. The block id Martin quoted appears once in
the log, inside the `creates_ids` of the ORGSYNC_DIFF for one of those four
files — tying the toast to the four WARNs. The 4x is NOT a retry loop:
disclosure is deduped once per `shared_tree_id` per session
(`di.rs:816-822`, set built once at `:563-565`) and the bus upserts by
`(subject, kind)` (`degraded_signal_bus.rs:238-248`), so four lines mean
four renders of one sticky toast or four distinct trees. CONSEQUENCE: the
edit is durable in Loro and SQL and syncs to peers, but the on-disk org
projection never catches up — there is NO retry and NO flush, and a
repo-wide search found no site that clears
`SHARED_SUBTREE_NOT_MATERIALIZED`, so the condition holds for the whole
session (`degraded_signal_bus.rs:88-91`). Correctly disclosed, hence not P1,
but it is a real disk-durability hole dressed as a warning. ENVIRONMENT: no
test restarts an app with a shared subtree on disk, so the
registration-vs-marker asymmetry has no coverage. DISCLOSED CASUALTY — this
one was NOT reproduced live: setting up a real share and restarting to lose
the registration was out of scope; the row is root-caused from the code plus
the log signature, and the missing reproduction is the work still owed.
Evidence:
`docs/Testing/fixture-logs-2026-08-08/triage5-log-signatures-integration-and-share.txt`
§4b)

## Missing piece

no test restarts an app with a shared subtree on disk, so the
marker-survives / registration-does-not asymmetry has no coverage

## Remedy

OPEN — P2, triage only, no fix in this lane; evidence
`docs/Testing/fixture-logs-2026-08-08/triage5-log-signatures-integration-and-share.txt`
§4b
