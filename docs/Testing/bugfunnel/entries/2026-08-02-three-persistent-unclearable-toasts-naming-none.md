---
id: 2026-08-02-three-persistent-unclearable-toasts-naming-none
date: 2026-08-02
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  Three PERSISTENT, unclearable `Shared edit saved — org file pending` toasts
  naming `block:661368d9…`, `block:9670e586…`, `block:240acff4…`. None of
  those blocks is part of a live share: they are ordinary headlines in
  `Projects/Holon/_archive/Phase 1: Core Outliner.org`, `…/Phase 2: First
  Integration (Todoist).org` and `…/Phase 6: Flow Optimization.org` that carry
  a STALE `:shared-tree-id:` drawer property left over from an earlier share
  experiment — 111 blocks across 4 distinct `shared-tree-id`s in the vault
  today. MECHANISM: `disclose_unmaterialized_share`
  (`crates/holon-orgmode/src/di.rs:764-830`) fires for ANY block whose
  `shared_tree_id()` is `Some` when the walk to its owning page does not
  terminate at an `is_share_mount()` PAGE. The only mount-marked block in the
  vault (`block:0c8d4eb7…`, `:share-role: mount:`) is NOT `Page`-tagged, and
  the other three tree-ids have no mount block at all, so the predicate can
  never be satisfied — the disclosure is raised on EVERY boot, deduped once
  per `shared_tree_id` per session, and the only way to remove it is the
  manual `✕` on the toast (`render_toast_stack`,
  `frontends/gpui/src/share_ui.rs:1597`). Seam: `ShareDegradedDisclosure`
  (`crates/holon-app/src/loro_seams.rs:635`). The gate keys on a
  USER-AUTHORABLE drawer property (any org file can contain
  `:shared-tree-id:`) while the ingest-skip decision one layer away already
  refuses to trust exactly that (`probe_share_file` keys on the authoritative
  `MountRegistry`,
  `crates/holon-filesystem/src/file_sync_controller.rs:1795-1810`, and the log
  shows it correctly classifying all three files as `share-role drawer … NOT a
  registered shared-subtree mount`,
  `/private/tmp/holon-cold.log:991,1065,1092`). Related to the still-OPEN
  2026-07-21 row on recipient-side share write-back.
source_line: 1134
---

## Bug

(dogfood, live GPUI on the real vault) Three PERSISTENT, unclearable `Shared
edit saved — org file pending` toasts naming `block:661368d9…`,
`block:9670e586…`, `block:240acff4…`. None of those blocks is part of a live
share: they are ordinary headlines in `Projects/Holon/_archive/Phase 1: Core
Outliner.org`, `…/Phase 2: First Integration (Todoist).org` and `…/Phase 6:
Flow Optimization.org` that carry a STALE `:shared-tree-id:` drawer property
left over from an earlier share experiment — 111 blocks across 4 distinct
`shared-tree-id`s in the vault today. MECHANISM:
`disclose_unmaterialized_share` (`crates/holon-orgmode/src/di.rs:764-830`)
fires for ANY block whose `shared_tree_id()` is `Some` when the walk to its
owning page does not terminate at an `is_share_mount()` PAGE. The only
mount-marked block in the vault (`block:0c8d4eb7…`, `:share-role: mount:`)
is NOT `Page`-tagged, and the other three tree-ids have no mount block at
all, so the predicate can never be satisfied — the disclosure is raised on
EVERY boot, deduped once per `shared_tree_id` per session, and the only way
to remove it is the manual `✕` on the toast (`render_toast_stack`,
`frontends/gpui/src/share_ui.rs:1597`). Seam: `ShareDegradedDisclosure`
(`crates/holon-app/src/loro_seams.rs:635`). The gate keys on a
USER-AUTHORABLE drawer property (any org file can contain
`:shared-tree-id:`) while the ingest-skip decision one layer away already
refuses to trust exactly that (`probe_share_file` keys on the authoritative
`MountRegistry`,
`crates/holon-filesystem/src/file_sync_controller.rs:1795-1810`, and the log
shows it correctly classifying all three files as `share-role drawer … NOT a
registered shared-subtree mount`,
`/private/tmp/holon-cold.log:991,1065,1092`). Related to the still-OPEN
2026-07-21 row on recipient-side share write-back.

## Missing piece

The keystone cannot generate the input. `shared_tree_mount_file`
(`crates/holon-integration-tests/src/pbt/generators.rs:747-780`) is the ONLY
arm that mints `:shared-tree-id:`, and it is gated behind
`HOLON_PBT_SHARED_TREE_MOUNT=1` (off in CI, because the reference model does
not track share properties), so no default run ever ingests a shared-tree
block. Even with the gate on it emits a mount WITH its children — never the
STALE shape here (a tree-id whose mount is absent or is not a page), which
is what makes the disclosure permanent. Secondary ORACLE:
`DegradedSignalBus` output is not an observable of the composed keystone, so
there is no invariant of the form 'a boot that only INGESTS org files raises
no degraded disclosure' — even a generated case would have passed silently.
Rungs that would close it: (a) un-gate the mount generator by teaching the
reference model `share-role`/`shared-tree-id`, and add a stale-share arm
(tree-id present, mount absent / mount not `Page`-tagged); (b) add an
invariant asserting the degraded-signal bus is EMPTY after a pure-ingest
settle.

## Remedy

OPEN — diagnosis only (2026-08-02 triage lane). Fix direction: decide the
disclosure's authority the same way ingest already did — gate
`disclose_unmaterialized_share` on `MountRegistry::is_registered_mount` for
the block's `shared_tree_id`, not on the drawer property, so a
stale/hand-authored `:shared-tree-id:` is inert. A tree-id with no
registered mount is not a pending write-back; it is a leftover property, and
at most deserves one WARN, not a standing banner. Needs Martin's ruling on
whether such stale properties should additionally be stripped from the
vault.
