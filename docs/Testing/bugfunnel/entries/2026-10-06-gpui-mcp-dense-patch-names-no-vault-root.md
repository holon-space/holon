---
id: 2026-10-06-gpui-mcp-dense-patch-names-no-vault-root
date: 2026-10-06
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  In the GPUI desktop app, every MCP `dense_patch` failed with "this session
  names no vault root", because no boot path gave `DebugServices` the vault root.
---

## Bug
The orchestrator found it while dogfooding through the GPUI app's embedded MCP
server: agents could not write decision blocks into Martin's vault. Every
`dense_patch` call failed with "this session names no vault root, so no row can
be checked against the file that holds it" (`frontends/mcp/src/tools.rs`,
`own_file`).

## Root cause
`dense_patch` reads the vault root from `DebugServices`. Only the standalone MCP
binary set it, in its own `on_start`. GPUI desktop (`frontends/gpui/src/main.rs`)
and mobile (`frontends/gpui/src/mobile.rs`) filled only the `live_debug` cell.
`holon_mcp::di::DebugServicesPopulatorModule` would have set the root, but nothing
constructed it. Six hand-written copies of the populate logic existed (standalone
binary, GPUI desktop, mobile, reset builder, `TestEnvironment`, the frontend-slice
component), and they differed. The frontend-slice copy
(`HeadlessFrontendComponent::mcp_debug_services`) set the root by hand, so the
keystone's dense-tools rung and `dense_patch_engine_exact` passed.

Red with the test component on GPUI's populate logic:
`a_retitle_keeps_the_body` and `a_body_edit_is_written` fail with this exact
error (lane-logs/red.log of lane gpui-mcp-root).

## Missing piece
The tests did not boot `DebugServices` through the app's wiring. They built their
own copy and set the root by hand.

## Remedy
One populator, `holon_mcp::di::populate_debug_services`, runs in every boot path:
GPUI desktop, mobile, the standalone binary, `TestEnvironment` and the frontend-slice
component (which now resolves the DI singleton). The reset builder uses its
`debug_handles`. The root comes from the session's `OrgModeConfig` and lives in the
swappable `DebugHandlesCell`, so a `reset_vault` swaps it with the session.
The unused module and the boot-only `orgmode_root` and `loro_doc_store` fields
are gone.
