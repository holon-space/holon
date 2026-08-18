---
id: 2026-08-18-cold-boot-discloses-shared-edit-for-every-share
date: 2026-08-18
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  On a cold boot the "Shared edit saved — org file pending" banner is raised
  once for EVERY shared subtree in the vault although no edit occurred — the
  boot-snapshot exclusion only covers blocks already in the feed at `Reset`,
  so blocks the org initial scan creates from disk are treated as live edits.
source_line: null
---

## Bug

Martin dogfooding the GPUI desktop app (cold boot, 2026-08-18): FOUR yellow
banners `Shared edit saved — org file pending`, naming block ids
`419b2df8…`, `7bc5f362…`, `f37ab7bc…`, `90702048…`. Martin had not edited
those blocks; three of the four files are under `Projects/Holon/_archive/`.

MEASUREMENT (read-only, Martin's vault + `/private/tmp/holon-cold.log`):

- The four ids live in FOUR DIFFERENT files, none of them the quarantined
  `Templates/Compass.org`, so this is NOT downstream of
  `2026-08-18-shipped-compass-asset-refused-by-own-parser`:
  `Projects/Holon/LogSeq replacement.org`,
  `Projects/Holon/_archive/Phase 1: Core Outliner.org`,
  `Projects/Holon/_archive/Phase 2: First Integration (Todoist).org`,
  `Projects/Holon/_archive/Phase 6: Flow Optimization.org`.
- Each of the four files carries a DISTINCT `shared-tree-id`, and those are
  the ONLY four distinct `shared-tree-id` values in the whole vault. **Four
  banners = four shares = every share in the vault.** The count tracks the
  number of shares, not the number of edits.
- Each of the four block ids appears EXACTLY ONCE in the 16 079-line log, and
  in every case inside the `org.initial_scan.ingest` → `org.ingest_file`
  ORGSYNC_DIFF for its own file (`holon-cold.log:586`, `:823`, `:856`,
  `:877`), e.g. `[ORGSYNC_DIFF] …/LogSeq replacement.org old=0 new=14
  create…`. The ids appear in NO live-edit path. The trigger is boot ingest.
- The log contains ZERO occurrences of `share-role drawer property was found
  on a page that is NOT a registered shared-subtree mount`, so the root cause
  recorded in `2026-08-08-disk-marker-survives-restart-share-mount` (lost
  mount REGISTRATION, corroborated there by exactly 4 such WARNs) does NOT
  hold for this occurrence. Three of the four files carry no `share-role`
  drawer at all; only `Phase 6: Flow Optimization.org:5` has
  `:share-role: mount`.

## Root cause

The disclosure fires on content that was just READ FROM DISK, which by
construction cannot have a write-back gap.

`disclose_unmaterialized_share` (`crates/holon-orgmode/src/di.rs:918-964`)
discloses when a block carrying `shared_tree_id()` walks up to an owning page
that is not `is_share_mount()`, deduped once per `shared_tree_id` per session
(`di.rs:948-953`). It is called from the block-feed task at `di.rs:661`, and
that call site is guarded by a `seeding` flag whose stated intent is exactly
to suppress this at startup (`di.rs:655-660`):

```
// Boot-snapshot blocks do not disclose: the gap is a property of the
// share's wiring, not of any one edit, and announcing it once per
// pre-existing block at startup is banner spam.
```

The guard fails its own contract on a cold boot. `seeding` is
`snapshot_pending.remove(&key)` (`di.rs:648`), and `snapshot_pending` is
populated ONLY inside the `Supervised::Reset` arm, from
`feed.read().keys().cloned()` (`di.rs:640-641`). `Reset` "is emitted before
*every* stream, including the first"
(`crates/holon-api/src/live_data/supervision.rs:20,81`) — i.e. at
subscription, BEFORE the org initial scan has ingested anything. On a cold
boot the store is empty, so the snapshot is empty, so `snapshot_pending` is
empty. Every block the initial scan then creates from disk arrives as an
ordinary post-snapshot `Upsert` with `seeding == false` and discloses.

That is why this is a COLD-boot signature: on a warm boot the blocks are
already in the feed at `Reset`, land in `snapshot_pending`, and are correctly
suppressed.

Two consequences beyond the banner text:

- The banner MISATTRIBUTES. It says "Shared edit saved", but no edit was
  saved; the content came from disk. The underlying condition ("these four
  subtrees own no dedicated file and cannot materialize") is nevertheless
  TRUE and worth disclosing — the walk in `di.rs:930-944` only accepts a
  mount that `is_page()`, while the one mount actually authored in the vault
  (`Phase 6…org:5`) is a HEADLINE carrying `:share-role: mount`, so the walk
  terminates at a non-mount page. `di.rs:913-916` documents this as
  self-disarming "post-Inc-2", and Inc 2 has not landed.
- The detail rendered to the user is a bare block id
  (`frontends/gpui/src/share_ui.rs:324-327`, `:1725-1729`), which names
  nothing a user can act on. The file or share name would.

The same `seeding == false` verdict also routes these ingest-created blocks
to `OrgRerender::Block` rather than `OrgRerender::Seed` (`di.rs:667`),
i.e. a cold boot write-back-renders each freshly ingested block individually
— the exact per-block boot render the `Seed` variant exists to avoid
(`di.rs:747-753`). NOT separately measured in this lane; flagged.

## Missing piece

The keystone PBT has NO share coverage at all: a search for
`share_role` / `SHARE_ROLE` / `shared_tree_id` across
`crates/holon-integration-tests/src/` returns zero hits, so no generated case
can produce a vault containing a shared subtree, and the cold-boot-vs-warm-
boot distinction that decides `seeding` is never exercised. Secondarily, the
disclosure seam is wired only in `crates/holon-app/src/wiring.rs:267` and the
banner surface is GPUI-only, so even a share-carrying case would disclose to
a WARN log rather than to an assertable banner in the headless harness.

## Remedy

FIXED (Martin's ruling D5B-10.a plus the accepted refutation of the id-set
approach, 2026-08-18). The disclosure MOVED out of the block-feed projection
and onto the WRITE-BACK path.

An id-set fix was built and measured first, and it does NOT hold: boot
re-projects the same block more than once, so a suppression set with a bounded
lifetime always loses the race. Instrumented, one block, two upserts in one
boot:

```
PROBE-DI key=block:share-child-two inset=true
PROBE-DI key=block:share-child-two inset=false
PROBE-DISCLOSE block=block:share-child-two stid=1111...
```

Taking the id let the second upsert disclose; peeking and clearing at
`finish_initial_scan` only moved the race. Both variants were flaky across
five repeat runs. That whole family — `AtomicBool` scan flag, scan-id set,
generation counter — infers edit-ness from diff traffic and cannot win.

The disclosure now lives at `FileSyncController::disclose_share_inlined_into`
(`crates/holon-filesystem/src/file_sync_controller.rs`), called from
`write_back_or_skip_readonly` — the single funnel through which every
projection write reaches the filesystem. It fires only after a real write, and
asks the authoritative `MountRegistry` whether the document is the share's own
mount file. Cold-boot ingest makes no write attempt (reading a file produces
no write, and an unchanged render is echo-suppressed), so a cold boot is
silent by construction rather than by suppression. Deduped per
`shared_tree_id`: one wiring gap, one banner.

The old feed-path `disclose_unmaterialized_share` and its unit tests are
DELETED, not left beside the new path.

B2 falls out of the move: the write-back layer knows the path, so the banner
reads `Shared subtree not materialized — <file>`, carrying a typed
`SharedSubtreeNotMaterialized { file }` rather than a pre-formatted string
(`crates/holon-loro/src/degraded_signal_bus.rs`,
`frontends/gpui/src/share_ui.rs`).

Rung: `crates/holon-integration-tests/tests/cold_boot_share_disclosure.rs`,
three cases — cold boot over a vault holding a shared subtree discloses
NOTHING; a WARM restart over the same vault (store already populated) also
discloses nothing; and a genuine STORE-side edit inside that share still
discloses. The warm case runs a real second boot, not a no-op restart: 36
initial-scan events were counted during it. The teeth test had
to be re-authored: the original drove the edit by writing the org file, which
under write-back semantics is already ON disk and correctly produces no gap.
Red for the right reason with the disclosure disabled:

```
a genuine store-side edit to a share with no page-mount MUST still disclose
  left: []
 right: ["11111111-2222-3333-4444-555555555555"]
```

Green, and 5/5 stable across repeat runs — the flakiness that condemned the
id-set approach is gone.

RESIDUAL, unchanged by this fix: the underlying condition is still TRUE for
Martin's vault. The mount walk accepts only `is_page()` mounts while his only
authored mount is a HEADLINE carrying `:share-role: mount`, so those subtrees
genuinely own no file. Inc 2 (tagging the mount a page) is what clears it;
this fix corrects WHEN and HOW the condition is announced, not the condition.

Still deferred: the per-block `OrgRerender::Block` cold-boot routing
(`di.rs`) is a latency-lane candidate. `seeding` drives both that routing and
(formerly) the disclosure, so whoever fixes the routing touches the same
predicate — the disclosure no longer depends on it.
