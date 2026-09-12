---
id: 2026-09-12-switching-an-integration-emits-no-latency-stage-so-its-slo-cannot-be-measured
date: 2026-09-12
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  Twenty integration set_field operations produced zero holon_latency events, so
  the p95 interaction→projection-visible SLO is unmeasurable for the settings
  rows even though the projection demonstrably runs.
---

## Bug

Found by the `dogfood-explorer` gate for `user-connections` (main
`f134df9ece6c`), while trying to report the p95 the gate is asked for.

The app was launched with `RUST_LOG=info,holon_latency=debug`, which is what
makes the stages emit. Twenty `execute_operation` calls of
`integration.set_field` (`enabled` true/false) were driven through the MCP
server. The work happened — the log gains 38 `[IntegrationStateProjector]
projected the enablement store into integration_state` lines, and the state file
on disk flips with each call.

`scripts/measure_latency.py` over that log
(`scratchpad/dogfood-uc/logs/latency-app6-after.txt`) reports the same three
stages as before the twenty operations: one cold-boot `projection`, one
`projection (snapshot only)`, two `rows`. Not one stage is attributable to a
toggle. Grepping the log for `holon_latency stage=` after the operations returns
only `matview_ddl` events from watch-view creation.

So the SLO cannot be evaluated on this surface. For a number that is not the
SLO: the MCP round trip including the per-call handshake measured p50 81 ms,
p95 83 ms, max 83 ms over 12 calls, which is comfortably inside 200 ms and is
consistent with no latency problem existing here. That is a proxy, and it is
reported as one — it includes transport the SLO excludes and excludes paint the
SLO includes.

**No latency violation was observed. What is reported is that a violation could
not have been observed.**

## Root cause

The `integration.set_field` path does not instrument itself. The `holon_latency`
stages are emitted by the block/projection pipeline; the enablement store writes
a TOML file and re-projects through `IntegrationStateProjector`, and neither end
of that opens a latency span.

This is consistent with the dogfood skill's own standing caveat that e2e stage
coverage is partial — `set_field` on a BLOCK emits an `e2e` stage, `split_block`
emits only `dispatch`, and the named `projection` stage never fires. The
integration row is a further surface with no coverage at all, and it is one a
user interacts with directly.

## Missing piece

ORACLE, by the bug-gap-triage rule that latency escapes are ORACLE or
ENVIRONMENT and never PERCEPTION. The interaction is trivially generatable — the
MCP operation exists and works — and no invariant could fire, because the budget
invariant has no stage to read. An SLO that cannot be measured on a surface is
not being held on that surface.

The keystone PBT cannot reproduce this; it does not drive integration rows.

## Remedy

PARTLY FIXED in lane `uc-fixes` (wave 12). The measurement above stands; the
root cause stated above does NOT, and correcting it is the main content here.

**The root cause was mis-stated.** The twenty operations were driven through the
MCP server, which calls `HolonService::execute_operation` ->
`BackendEngine::execute_operation` — below every frontend dispatch seam. The
seams that open the interaction clock
(`holon_frontend::reactive::dispatch_intent` and
`operations::dispatch_operation`) are above that, so an MCP-driven op opens no
clock for ANY surface, block operations included. The `integration.set_field`
path is not uninstrumented; it was not driven through the instrumented seam.

Measured, not reasoned:
`frontends/gpui/tests/settings_integration_toggle_latency_windowed.rs` clicks
the real Settings switch with a real mouse event, waits for the mirror to flip,
and asserts `latency_e2e::pending_targets()` no longer holds
`integration:claude-history`. It passes
(`lane-logs/item5-GREEN-1789180650.log`), so a user's click on that switch does
open an end-to-end sample and the sample does close at the projection.

**One real gap, fixed.** `dispatch_intent` — the fire-and-forget path every GPUI
click uses — emitted no `stage="dispatch"` event, though its awaiting sibling
`dispatch_intent_sync` always has. So a clicked interaction reported its total
and nothing about where the time went. `crates/holon-frontend/src/reactive.rs`
now emits it, with the op's target as `block`.

**Still open, and this is the part for Martin.** A programmatic driver — the
dogfood explorer, an MCP-driven fixture — cannot measure any interaction,
because the only instrumented seams are UI-side. Either `HolonService` opens a
clock for the ops it dispatches (making "interaction" mean "op from a session
facade"), or the dogfood skill states that latency must be read from a
click-driven windowed rung and not from an MCP session. The lane did not decide
this. The broader question the entry already raises — which surfaces the SLO
binds — is unchanged and also open.

## Attribution

NOT a regression of `user-connections`, and not an integration-specific defect
either. `dispatch_intent` has lacked the dispatch stage since it was written;
the MCP driver has always been below the seam. The lane is what put a
user-facing switch on a surface nobody had measured, which is how it surfaced.

One incidental measurement worth keeping: a Settings row far enough down the
modal is clipped by the window, and its tracked box comes back 80x0 at y=1121,
so a windowed rung cannot click it. The rung above uses the first row by
`provider_name ASC`. Driving a lower row needs a scroll no windowed test
performs today.
