---
id: 2026-07-16-org-write-back-feed-dies-silently
date: 2026-07-16
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  Org write-back feed DIES silently mid-session: after the Projects.org
  grounding-error storm (17:32), ZERO `org.on_block_changed` events for ANY
  later DB mutation (rule create, move_block, set_field) — DB and vault files
  silently diverge, no banner, app looks healthy
source_line: 819
---

## Bug

Org write-back feed DIES silently mid-session: after the Projects.org
grounding-error storm (17:32), ZERO `org.on_block_changed` events for ANY
later DB mutation (rule create, move_block, set_field) — DB and vault files
silently diverge, no banner, app looks healthy

## Missing piece

no liveness invariant on the DB→org feed; small-vault tests never trigger
the error storm that kills it (fresh small-vault instance writes back fine)

## Remedy

FIXED (2026-07-17). ROOT CAUSE was NOT the org controller (its `select!`
loop logs `on_block_changed` errors and continues) — it was the UPSTREAM
`LiveData::apply_changes` (`crates/holon-api/src/live_data.rs`): three panic
sites (`.expect("id_fn failed on CDC row")`, `.expect("parse_fn failed on
CDC row")`, `panic!("unexpected FieldsChanged")`). A single
un-keyable/un-parseable block CDC row panicked the DETACHED `subscribe`
actor task; tokio swallows that panic (no test panic hook in prod), the CDC
stream is never drained again, and EVERY downstream mirror off the shared
`BlockFeed` (org write-back resolver, link indexer, ViewModel) silently
freezes — exactly "feed dead, app looks healthy, DB and files diverge." This
is the "silently degrades to look fine" anti-pattern (priority 4) the repo
forbids. FIX: the three panics become loud `tracing::error!` +
DROP-this-row-and-continue (disclosed degradation, priority 2) — one bad row
no longer kills the feed. ENV-GAP closed on the observability side: a
WARN-level "LiveData stream ended" was below the `inv-no-observed-errors`
ERROR bar (structurally invisible); the swallowed panic was invisible in
prod but IS caught by the PBT panic hook. RED-first repros
(`crates/holon-api/src/live_data.rs` tests):
`apply_changes_drops_unparseable_row_keeps_feed_alive` (batch-level) +
`subscribe_actor_survives_unparseable_row` (actor-level: poison row then
good row over a real stream — pre-fix the actor dies and the good row never
lands). COVERAGE gap remaining: no integration test drives a malformed block
CDC row through the full app feed at vault shape.
