---
id: 2026-07-21-mcp-tool-always-targets-phantom-500x500
date: 2026-07-21
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  MCP `screenshot` tool always targets a phantom 500x500 title="" window on
  the holon-gpui pid and fails ("minimized=true") instead of the real
  3440x1440 "Holon" window — its CGWindowID picker selects the wrong window.
source_line: 1078
---

## Bug

MCP `screenshot` tool always targets a phantom 500x500 title="" window on
the holon-gpui pid and fails ("minimized=true") instead of the real
3440x1440 "Holon" window — its CGWindowID picker selects the wrong window.

## Missing piece

The keystone never enumerates OS windows nor drives the macOS screenshot
path — a platform-only MCP tool wiring the test env cannot exercise (same
drive-layer class as the other MCP tooling rows). Remedy: pick the
CGWindowID by real title/size (skip empty/zero-title and sub-threshold
windows), fail loud when no real window matches.

## Remedy

OPEN — 2026-07-21 dogfood tooling.
