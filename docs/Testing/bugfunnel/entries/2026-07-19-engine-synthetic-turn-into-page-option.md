---
id: 2026-07-19-engine-synthetic-turn-into-page-option
date: 2026-07-19
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Engine-synthetic `convert_block_to_page` ("Turn into page", Option B) never
  appeared in the GPUI slash menu, op-button toolbar, or any keyboard surface
  — the op was reachable only via MCP `available_operations` /
  `execute_operation`, never from the UI. ROOT CAUSE: the profile-op source
  that feeds every op-driven UI surface — `entity_operations` in
  `create_profile_resolver` (`crates/holon/src/di/registration.rs`) →
  `resolve_profile(row).operations` →
  `build_command_items`/`ops_of`/chord-pump — is built ONLY from
  `dispatcher.operations()`. The two synthetic descriptors
  (`convert_block_to_page` always; `instantiate_template` when a template
  source exists) are appended solely inside
  `OperationEngine::available_operations` (`operation_engine.rs`), which the
  profile path never calls. So the MCP discovery surface advertised the op
  while the UI did not — a divergence between two op-source paths. Found via
  code trace + live confirmation (`include_profile` showed the block profile
  op list ending at `…add_tag,remove_tag` with no `convert_block_to_page`)
source_line: 802
---

## Bug

Engine-synthetic `convert_block_to_page` ("Turn into page", Option B) never
appeared in the GPUI slash menu, op-button toolbar, or any keyboard surface
— the op was reachable only via MCP `available_operations` /
`execute_operation`, never from the UI. ROOT CAUSE: the profile-op source
that feeds every op-driven UI surface — `entity_operations` in
`create_profile_resolver` (`crates/holon/src/di/registration.rs`) →
`resolve_profile(row).operations` →
`build_command_items`/`ops_of`/chord-pump — is built ONLY from
`dispatcher.operations()`. The two synthetic descriptors
(`convert_block_to_page` always; `instantiate_template` when a template
source exists) are appended solely inside
`OperationEngine::available_operations` (`operation_engine.rs`), which the
profile path never calls. So the MCP discovery surface advertised the op
while the UI did not — a divergence between two op-source paths. Found via
code trace + live confirmation (`include_profile` showed the block profile
op list ending at `…add_tag,remove_tag` with no `convert_block_to_page`)

## Missing piece

No test asserts that engine-synthetic ops reach
`resolve_profile(row).operations` / the slash-menu command list; the
keystone builds command items from the same `dispatcher.operations()`-only
`entity_operations`, so it could not generate a "Turn into page" slash entry
either (the entry was structurally absent, not just unasserted). Secondary
ENVIRONMENT: two op-source paths (`available_operations` vs profile
`entity_operations`) diverged with no parity check

## Remedy

FIXED 2026-07-19: `create_profile_resolver` now appends
`DispatchingOperationEngine::convert_block_to_page_descriptor()` to
`entity_operations["block"]` (the same descriptor `available_operations`
uses — single source), so convert reaches the slash menu (confirmed live:
screenshot shows "Turn into page" in the popup), the op-button toolbar, and
is available beside indent/outdent everywhere. `instantiate_template`
intentionally NOT added (needs a template pick; surfaced via the template
picker, and `build_command_items` hides the bare op). Keyboard surface added
separately (`TurnIntoPage` editor action, Cmd/Ctrl+Shift+P,
`frontends/gpui`), live-verified end-to-end (minted a `Page`-tagged block +
left a `Link` mark on the origin). Pinned by
`command_provider::tests::convert_block_to_page_surfaces_as_turn_into_page`
