---
id: 2026-07-20-gpui-slash-menu-shows-only-turn
date: 2026-07-20
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  GPUI slash menu shows ONLY "Turn into page" on a normal block (no
  indent/outdent/move/delete): `operation_matcher::filter_by_intent_params`
  treated every `param_mapping.from` as an intent-param source and, when any
  was present in context, kept only ops mapping from a present source.
  `convert_block_to_page` is the ONLY block op with a param_mapping and it
  maps `from:"id"` (operation_engine.rs); context always carries `id`, so the
  filter dropped every op without an `id` mapping. Regression introduced by
  the convert feature (383a031824) — before it, no op mapped from `id`, so
  `present` was empty and ALL ops showed.
source_line: 1025
---

## Bug

GPUI slash menu shows ONLY "Turn into page" on a normal block (no
indent/outdent/move/delete): `operation_matcher::filter_by_intent_params`
treated every `param_mapping.from` as an intent-param source and, when any
was present in context, kept only ops mapping from a present source.
`convert_block_to_page` is the ONLY block op with a param_mapping and it
maps `from:"id"` (operation_engine.rs); context always carries `id`, so the
filter dropped every op without an `id` mapping. Regression introduced by
the convert feature (383a031824) — before it, no op mapped from `id`, so
`present` was empty and ALL ops showed.

## Missing piece

slash-menu population ran headlessly
(`build_command_items`/`find_satisfiable`) but NO invariant asserted a
normal block's command menu contained the expected structural ops.

## Remedy

**FIXED THIS LANE (2026-07-20)**: (A) `filter_by_intent_params` now excludes
the universal `id`/`id_column` from gesture-intent sources. Detection
installed permanently: (1) `operation_matcher` unit test
`universal_id_mapping_does_not_filter_out_plain_ops` (real macro block
descriptors, red→green); (2) **Inc1 registry↔menu correspondence** —
non-defaultable `MenuExposure` classification on `OperationDescriptor` +
single-sourced `block_synthetic_descriptors` catalog + holon-app test
`slash_menu_equals_the_listed_ops_resolvable_from_id_context` asserting the
menu == exactly the `Listed` ops (red showed `{convert}` vs the 8-op Listed
set). `build_command_items` now filters to `Listed`, so gesture/internal ops
also stop leaking. Inc2/Inc3 (keystone menu-open + inverse-execute rungs)
deferred
