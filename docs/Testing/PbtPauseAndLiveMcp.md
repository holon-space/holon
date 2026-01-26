# Pausing PBTs for live MCP inspection

When a property-based test (PBT) trips an invariant, the process panics
and the embedded `holon` MCP server tears down with it — the live DB,
Loro tree, and CDC state that produced the failure are gone before
anything can attach. This page documents the env-var-driven hooks that
hold the process open at chosen moments so an external tool (the
`holon-live` proxy, a debugger, or a sqlite client) can attach and
inspect.

## Quick start

```bash
PROPTEST_SEED=42 PBT_PAUSE_SECONDS=300 \
  cargo test -p holon-gpui --test gpui_ui_pbt --features pbt
```

When the PBT panics, you'll see:

```
═══════════════════════════════════════════════════════════════════
[PBT_PAUSE_SECONDS (panic hook)] panic at crates/.../assertions.rs:133:
  assertion `left == right` failed: Org file block ordering wrong: ...
PID: 15758    Sleeping: 300s    SIGINT aborts.
Connect via the holon MCP server (port 8528), attach a debugger, or
open the test sqlite DB to inspect live state.
═══════════════════════════════════════════════════════════════════
```

You then have 5 minutes (or whatever you set) to query the embedded
MCP server on port 8528 before the process tears down. `Ctrl+C` aborts
the sleep immediately.

## Env vars

| Variable | Effect |
|---|---|
| `PBT_PAUSE_SECONDS=N` | **Master switch.** When set: (a) installs a global panic hook that sleeps `N` seconds before any panic propagates, and (b) forces the embedded MCP server up on port **8528** (`MCP_PAUSE_PORT`) regardless of `PBT_MCP_PORT`. When unset, both are no-ops. |
| `PBT_PAUSE_BEFORE_STEP=N` | Sleep immediately before applying transition `N` (1-based, matches the `[pbt_step] Step N/M` log line). Useful with a debugger to set breakpoints before an operation fires. Honors `PBT_PAUSE_SECONDS` for duration. |
| `PBT_PAUSE_AFTER_STEP=N` | Sleep immediately after transition `N`'s invariants are checked. |
| `PBT_MCP_PORT=8521` | Manual MCP port (overridden by `PBT_PAUSE_SECONDS` when both are set). |
| `PBT_KEEP_WINDOW=1` | Keeps the GPUI window open after the PBT thread finishes (independent of pauses). |

## Connecting `holon-live` MCP

The repo's `.mcp.json` includes a `holon-live` server entry. It's a
thin `mcp-rust-proxy` that talks HTTP to whatever MCP server is listening
locally. When the PBT pauses, port 8528 is up and `holon-live`'s tools
become available in the agent harness:

- `mcp__holon-live__holon_pbt__execute_raw_sql` — direct Turso queries
  (`SELECT … FROM block_raw WHERE …`, etc.).
- `mcp__holon-live__holon_pbt__inspect_loro_blocks` — read the Loro
  tree state for a document.
- `mcp__holon-live__holon_pbt__diff_loro_sql` — show mismatches between
  Loro's tree view and SQL's `block_raw` rows.
- `mcp__holon-live__holon_pbt__describe_ui` /
  `mcp__holon-live__holon_pbt__describe_navigation` — inspect what the
  frontend is rendering.
- Plus: `list_loro_documents`, `list_tables`, `compile_query`,
  `screenshot`, etc. (full list via the harness's MCP browser).

If `inspect_loro_blocks` returns *"Loro is not enabled in this session"*,
the test environment didn't wire the populated `DebugServices`. The
`PbtReadyContext.debug_services` field plumbs it through (registered in
`TestEnvironmentBuilder::start_app` via
`holon_mcp::di::register_debug_services` + the inline equivalent of
`DebugServicesPopulatorModule`). Both `gpui_ui_pbt` and `tui_ui_pbt`
forward this Arc into `try_start_embedded_mcp`.

If you spawn a new MCP-using PBT entrypoint, mirror that pattern.

## How the pause works under the hood

The mechanism lives in
[`crates/holon-integration-tests/src/debug_pause.rs`](../../crates/holon-integration-tests/src/debug_pause.rs):

- `install_panic_pause_hook()` is called by every PBT entry-point
  (`run_pbt_with_driver_sync_callback`, `run_phased_pbt_sync`,
  `pbt_setup`). When `PBT_PAUSE_SECONDS` is set, it installs a
  `std::panic::set_hook` that sleeps before chaining to the previous
  hook. The sleep is on the panicking thread; the embedded MCP server
  (running in its own tokio task) keeps serving requests during the
  pause.
- `try_start_embedded_mcp` (in `pbt/ui_harness.rs`) checks
  `pause_enabled()` and pins the port to `MCP_PAUSE_PORT = 8528` so
  external tools always know where to connect.

## Debugger workflow (LLDB / `debugger_mcp`)

For step-through debugging instead of pause-and-inspect:

```bash
# Pre-build with debug symbols
cargo test -p holon-gpui --test gpui_ui_pbt --features pbt --profile debugger --no-run

# The binary path is printed at the end of the build:
#   Executable tests/gpui_ui_pbt.rs (target/debugger/deps/gpui_ui_pbt-XXXX)
```

Then point `debugger_mcp` at that binary with `language: "rust"` and
your breakpoints. Conditional breakpoints work for filtering by string
contents (e.g.
`predecessor_id.is_some() && predecessor_id.unwrap().contains("c2f12z-s")`).

**Limitation:** LLDB's Clang-based expression evaluator can't dispatch
arbitrary Rust trait/inherent methods on this binary. If you need to
read complex Loro state (`tree.fractional_index`, `tree.children`, …),
add a temporary `eprintln!` instead — the debugger can hit the
breakpoint to confirm the right code path, but data extraction needs
the host language. Synthetic-children expansion of large Rust types
(HashMap, Vec, BTreeMap) can OOM the debug adapter — prefer
`debugger_get_variables` with `noSynthetic: true` for scalar reads.

## When to use which knob

| Want to … | Use |
|---|---|
| Inspect DB after an invariant trip | `PBT_PAUSE_SECONDS=N` (panic hook) |
| Inspect state mid-test at a specific step | `PBT_PAUSE_BEFORE_STEP=N` |
| Step through a specific operation in LLDB | `--profile debugger` + `debugger_mcp` |
| Keep the GPUI window open for visual diff | `PBT_KEEP_WINDOW=1` |
| Re-run with a known seed | `PROPTEST_SEED=N` |
