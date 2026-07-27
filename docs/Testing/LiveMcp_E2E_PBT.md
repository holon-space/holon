# Live-MCP E2E PBT — the keystone against a running iOS app

Status: **IMPLEMENTED + functional (2026-07-07)**. The composed keystone PBT
(`tests/general_e2e_composed_pbt.rs`) has a second, env-gated entry
`general_e2e_composed_pbt_live_mcp` that drives a LIVE Holon app over MCP —
verified against the iOS simulator, running GREEN.

Its first generated case originally diverged (a split dispatched at
end-of-text: `cursor=6` instead of `1`). That was **NOT** the deferred B1
iOS text-commit bug — it was a **live-driver focus race**: `focus_editor`
clicked a block and callers immediately sent `home`/`right`/`enter`, but on
iOS the click's editor focus lands a frame LATER (idle rendering — often the
keystroke itself drives the frame that applies focus), so the caret
keystrokes missed the editor and the split fired at the caret's stale
position. Fixed by making `focus_editor` wait on
`debug_pbt_snapshot.focused_block` (the engine's authoritative focus)
matching the clicked block before returning any caret keystrokes.

B1 proper — creation-slot parent = panel id (engine rejects), the unpushed
gpui-mobile `68df9dd` soft-keyboard Return fix, and the dead Focus/Blur
family (see memory `ios_b1_addblock_commit_rootcause_2026-07-07`) — is a
**separate, still-open** track. The split transitions this rung generates do
not exercise it.

## Shape (decided via Fable pressure-test; see session 4e66ccbb)

- **Env-pinned mode, not a wiring axis**: `HOLON_PBT_LIVE_MCP=1` selects the
  live entry; unset ⇒ disclosed skip. A live app has ONE fixed wiring; the
  captured live `CapSet` pins `init_state` (windowed-sibling pattern), so
  generation + non-vacuity floor auto-narrow.
- **Sibling slice** `LiveMcpE2E` + `WideE2ELiveMcpMachine`
  (`src/pbt/composed/live_mcp.rs`), same oracle / transitions / catalog as
  `WideE2E`; same source file for proptest-regression replay against the
  in-proc slice.
- **Per-case reset**: `reset_vault` with the `include_str!`-embedded
  `scripts/seed_wide/*.org` (byte-alignment unit test
  `seed_wide_stays_aligned`). ~0.85 s/reset, retire cap 20/launch, RSS grows
  linearly ~7.7 MB/reset (soak `scripts/soak_reset_rss.py`) ⇒ keep the cap,
  chunk via `scripts/run_live_mcp_pbt.sh` (cold relaunch between chunks;
  `max_shrink_iters: 0` — replay + shrink in-proc, never through resets).
- **Settle**: server-side `await_quiescence` MCP tool (debug-gated) — the
  combined fixed point (CDC watermark + Loro frontier + org idle) run
  in-process, fail-loud on budget; returns `lamport_height` for the clock feed.
- **Read caps over MCP** (existing async traits, honest impls):
  `SutBackend`/`SutSqlProjection` via `execute_raw_sql` (+ `debug_pbt_snapshot`
  for the LiveData mirrors), `SutLoroLog` via `debug_pbt_snapshot` +
  `inspect_loro_blocks`, `SutOrgRead` (alias-scoped org files, parsed
  client-side), `SutOrgRender` via the `render_org` tool at `source=sql`
  `scope=document` — the server's own write-back render, so the fixed point
  compares disk against the code that wrote it. Loro peer docs / reactive VM /
  editor mirror: honestly absent, auto-deselected.
- **Gesture caps**: production keystroke/click sequences over `McpUserDriver`
  verbs; navigation dispatches `navigation.focus` (phone-width sidebar is a
  closed drawer — no clickable bounds).

## Fixes this rung forced (all in this workspace)

- `DebugServices.live_debug` swappable handle cell (Loro sync handle, org idle
  signal, `BlockQuerySource` from `FrontendSession::block_query`, doc store) —
  populated at mobile boot, swapped by `reset_vault`.
- `current_loro_doc_store()` accessor in `frontends/mcp/src/tools.rs`: the old
  boot-only `OnceLock` was never populated on iOS and would go stale across
  resets; `list_loro_documents` / `inspect_loro_blocks` / alias resolution now
  follow resets.
- `FlushOnReadGeometry` (`frontends/gpui/src/geometry.rs`): the MCP driver
  reads element bounds from a window that paints once and goes idle on iOS —
  frame N's bounds sat in `staged` forever because no frame N+1 ever called
  `begin_pass`. Flush-on-read commits the last complete frame. Without this,
  every `click{entity_id}` on iOS fails with "no bounds recorded".

## How to run

```
# app on a booted sim, MCP on 8521, reset enabled:
HOLON_MCP_ALLOW_RESET=1 IOS_SIM_UDID=<udid> \
  crates/holon-integration-tests/scripts/ios_reset_sut.sh --port 8521

# one case:
HOLON_PBT_LIVE_MCP=1 MCP_SERVER_PORT=8521 PROPTEST_CASES=1 \
  cargo test -p holon-integration-tests --features pbt \
  --test general_e2e_composed_pbt live_mcp -- --nocapture

# chunked long run (cold relaunch between chunks):
HOLON_MCP_ALLOW_RESET=1 CHUNK=8 CHUNKS=2 \
  crates/holon-integration-tests/scripts/run_live_mcp_pbt.sh
```

Invariant softening (`HOLON_PBT_INVARIANTS="inv-...:warn"`) exists for
disclosed degraded runs, but is not needed here: with the focus-race fixed,
the split transitions run green end-to-end without softening.

## Follow-ups

- Fix B1 (iOS add-block/text commit) — a separate open track; the split
  transitions this rung generates do not exercise it (see status above).
- `describe_navigation` reads a stale reactive tree post-rebind (cosmetic here,
  same stale-handle class as the ones fixed above).
- The MCP-facing `GpuiUserDriver` still binds the boot engine (documented in
  `setup_interaction_pump`); click-intent resolution against a post-reset
  engine may need the same swappable-cell treatment if intent-verbs are added.
- Region/intent verbs still honestly panic on the MCP rung (needs region-aware
  `describe_ui`).
