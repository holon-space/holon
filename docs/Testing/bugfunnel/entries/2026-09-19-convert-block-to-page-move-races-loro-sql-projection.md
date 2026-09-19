---
id: 2026-09-19-convert-block-to-page-move-races-loro-sql-projection
date: 2026-09-19
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  convert_block_to_page re-homes children under a page it just minted, but the move's
  destination guard reads existence from the SQL projection, which in Loro-authority mode
  the create has not reached yet, so the compound fails with "Parent not found".
---

## Bug

The deterministic hand-authored keystone case
`journals-external-rewrite-strips-convert-link-marks` fails intermittently at transition
9/22, `BlockToPage(journals::auto-create)`:

```
convert_block_to_page: constituent 'move_block' failed: Parent not found: block:7b512cac-d64f-e768-7a50-ac1e5a8f25f8
```

Found by the 2026-09-18 landing gate, not by dogfooding. It is recorded here anyway
because the fix is a production change: the gate is the perception layer that failed to
classify a real prod race as anything other than a flake. Investigated by lane
`journals-rewrite-race`. Archived evidence:
`crates/holon-integration-tests/hand-authored-regressions/fixture-logs-2026-09-19/journals-rewrite-race-parent-not-found-excerpt.log.zst`.

## Root cause

`run_convert_block_to_page` (`crates/holon/src/api/operation_engine.rs:1175`) creates page
P at step 3, then at step 4 dispatches `move_block(child, parent_id = plan.page_id)` for
each child. The destination guard in the default `move_block`
(`crates/holon-core/src/traits.rs:2450-2461`) reads P's existence from the SQL projection
through `get_by_id`, and its own comment states the split: existence from the projection,
page-ness from the write authority.

In Loro-authority mode the create lands in Loro and reaches SQL only later, written by the
spawned wake-driven `loro-outbound-reconcile` task
(`crates/holon-loro/src/loro_sync_controller.rs:490`, `on_loro_changed` at `:544`,
`project()` at `:881`, doubling-backoff retry at `:555-599`). The dispatcher's block
provider is `SqlBlockOperations`
(`crates/holon-loro-wiring/src/event_infra_module.rs:160`), whose `get_by_id`
(`crates/holon/src/core/sql_block_operations.rs:254`) is an unwaited SQL read. So the
compound reads its own write across a boundary that write has not crossed yet. No settle
runs inside a compound; the harness settles only between transitions.

Measured. An env-gated delay at `on_loro_changed` and nothing else:

| Condition | Runs | Result |
|---|---|---|
| No lag, quiet | 3 | GREEN |
| No lag, 24 CPU loaders (load avg 174) | 6 | GREEN |
| No lag, full corpus, loaders (load avg 81) | 2 | GREEN |
| `HOLON_TEST_PROJECTOR_LAG_MS=600` on base | 6 | RED 5 of 6 |
| 500 ms at `project()`, a first probe since removed | 4 | RED, all 4 |

The 600 ms row is the honest combined figure: 3/3 red in this lane plus 2/3 red in an
independent verifier run, so the reproducer is strong but not certain on a single run.
The earlier `project()` probe was a scratch edit that no longer exists in the tree; the
only lag hook that ships is `HOLON_TEST_PROJECTOR_LAG_MS`.

Logs under the lane's `lane-logs/`: `runner-baseline2.log`, `runner-loaded.log`,
`runner-full.log`, `runner-onloro600.log`, `runner-lag500.log`. The page id in the injected
repro is byte-identical to the one in the 2026-09-18 gate log, so both concern the same
minted page. That gate log also shows interactions expiring after 30+ seconds waiting for a
delivered row, i.e. the projection starved far longer than the window this bug needs.

## Missing piece

Two, which is why this is ENVIRONMENT with a COVERAGE secondary.

The environment gap: the per-transition settle
(`crates/holon-integration-tests/src/pbt/composed/wide_e2e.rs:181`) converges all three
projections between transitions, so in the test the projection is always caught up before
the next transition begins. The race lives strictly inside one compound, where nothing
settles. The test environment therefore hides a timing seam that production has whenever
the projector is starved.

The coverage gap: the transition alphabet has no way to generate a lagging projection.
Nothing in the catalog can starve the reconcile task, so no generated sequence could ever
reach the failing state deterministically. Before this lane there was no fault-injection
rung at all for the Loro→SQL seam.

## Remedy

Implemented as a spike, landing pending the D148 ruling. Status stays `OPEN` because the
production fix has not landed.

The spike introduces a `WriteAuthorityReads` seam in `holon-core` (`block_exists` and
`block_is_page`), adds `exists_authoritative` to `BlockDataSourceHelpers` defaulting to the
projection, and routes the destination guard through both. `SqlBlockOperations` takes an
optional authority and defers to it; `LoroBlockOperations` answers from the live doc;
`event_infra_module.rs` resolves the SHARED instance. Page-ness needed moving too, not just
existence: `block_is_page` (`crates/holon/src/core/sql_operation_provider.rs:1212`) queries
`block_tags`, the same lagging projection.

BLAST RADIUS, wider than the one compound. The guard is the shared chokepoint, and the
`is_page_authoritative` override also changes where the MOVED block's page-ness comes from
(`moved_is_page`, `crates/holon-core/src/traits.rs:2434`), not only the destination's. On
the Loro path these now read the doc: `move_block`, `outdent`, `move_up` and `move_down`
(the last three reach the guard through `self.move_block` or a `new_parent: None`
prefetch). `indent` does NOT: it prefetches `new_parent: Some(prev_sibling.is_page())` and
never enters the lookup branch.

Verified: `cargo nextest run -p holon -p holon-app` shows a fail set identical to baseline
by name, keystone-smoke passes, and the 600 ms reproducer goes green. The class is broader
than this compound: any compound that creates a row and then references it through a
dispatched constituent had the same bug latent, and the shared guard now covers them.
