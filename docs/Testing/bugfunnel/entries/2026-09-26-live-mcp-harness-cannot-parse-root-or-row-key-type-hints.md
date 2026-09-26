---
id: 2026-09-26-live-mcp-harness-cannot-parse-root-or-row-key-type-hints
date: 2026-09-26
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  `just keystone-mcp` died at harness boot because `OperationParam`'s
  hand-written `TypeHint` deserializer rejected the `entity_id_or_root` and
  `row_key` variants that the derived serializer emits.
---

## Bug
A verifier probe of `just keystone-mcp 8720 16 '*:1,DenseProjectionEdit:100'`
against a live app (`HOLON_MCP_ALLOW_RESET=1 just live-verify 8720`) panicked
at harness boot: `refresh_ui after boot navigation failed: describe_ui(block:structural-page)
JSON did not deserialize as ViewModel ... Unknown type hint variant`. Reproduced
in lane `keystone-mcp-boot` (`lane-logs/kmb-repro2.log`).

## Root cause
The harness and the app already share one type: `McpUserDriver::refresh_ui`
parses `describe_ui` into `holon_frontend::view_model::ViewModel`. The shape
did not drift between them. The drift was inside `holon-api`: `TypeHint`
(`crates/holon-api/src/render_types.rs:557`) serializes through its derived
`#[serde(tag = "type", rename_all = "snake_case")]` impl, but
`OperationParam.type_hint` deserialized through a hand-written
`deserialize_type_hint` visitor that listed the variant names a second time.
`EntityIdOrRoot` and `RowKey` were added to the enum later and never to the
visitor, so every operation descriptor with a root-admitting reference
parameter made the whole view model
unparseable.

## Missing piece
No test round-tripped every `TypeHint` variant through JSON, and no gate runs
the live-MCP keystone rung (it needs a running app), so the only path that
deserializes served descriptors was never exercised.

## Remedy
Deleted the hand-written visitor; `OperationParam` now deserializes `TypeHint`
with the same derived impl that serializes it. Pinned by
`every_type_hint_round_trips_through_json` in `render_types.rs` (red:
`lane-logs/kmb-red-typehint.log`, green: `lane-logs/kmb-green-typehint.log`).
The keystone-mcp run now boots (`lane-logs/kmb-green-star0-16.log`).
