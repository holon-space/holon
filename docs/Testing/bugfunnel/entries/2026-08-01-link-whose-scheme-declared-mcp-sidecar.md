---
id: 2026-08-01-link-whose-scheme-declared-mcp-sidecar
date: 2026-08-01
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  A `[[<entity>:<id>]]` link whose scheme is declared by an MCP sidecar
  renders muted+wavy forever and produces no `block_links` row and no
  backlink, on every vault where the block was ingested before the provider
  connected — which is near-deterministically EVERY boot: `BackendEngine`
  resolves `FileSyncStarted` and spawns the org scan at
  `crates/holon-app/src/turso_seams.rs:898-905`, while entity types register
  only when the MCP provider CONNECTS
  (`crates/holon-app/src/mcp_integrations.rs:272-273`, whose factory is first
  resolved later, at `crates/holon-app/src/wiring.rs:333`).
  `LinkTargetClassifier` asks the live registry
  (`crates/holon-api/src/link_parser.rs:154-176`) and would answer correctly a
  second later, but its verdict is PERSISTED: `strip_link` writes
  `EntityRef::UnknownScheme` into the block's marks
  (`crates/holon-org-format/src/inline_marks.rs:583`, variant at
  `crates/holon-api/src/inline_mark.rs:50-70`), `derive_block_links` skips
  that variant so no junction row is ever created (`inline_mark.rs:362-368`),
  and the GPUI renderer decorates the STORED variant instead of re-classifying
  (`frontends/gpui/src/render/builders/text.rs:242-249`). Not self-correcting
  across restarts: the cold-boot fast path skips any file whose
  `sha256(RENDERER_VERSION ‖ consolidator ‖ disk_bytes)` is unchanged
  (`crates/holon-filesystem/src/file_sync_controller.rs:845-855, 1942-1966`)
  and the bytes never change, so the poisoned marks are permanent. Violates
  Model.md invariant 3 (projection reproducible from the replica: it encodes
  the registry contents at the ingest INSTANT, a hidden mutable input) and
  invariant 4 (derived holders recompute at quiescence). Found by agent
  exploration of the F2a provider lane, not by any test.
source_line: 1132
---

## Bug

(task #98) A `[[<entity>:<id>]]` link whose scheme is declared by an MCP
sidecar renders muted+wavy forever and produces no `block_links` row and no
backlink, on every vault where the block was ingested before the provider
connected — which is near-deterministically EVERY boot: `BackendEngine`
resolves `FileSyncStarted` and spawns the org scan at
`crates/holon-app/src/turso_seams.rs:898-905`, while entity types register
only when the MCP provider CONNECTS
(`crates/holon-app/src/mcp_integrations.rs:272-273`, whose factory is first
resolved later, at `crates/holon-app/src/wiring.rs:333`).
`LinkTargetClassifier` asks the live registry
(`crates/holon-api/src/link_parser.rs:154-176`) and would answer correctly a
second later, but its verdict is PERSISTED: `strip_link` writes
`EntityRef::UnknownScheme` into the block's marks
(`crates/holon-org-format/src/inline_marks.rs:583`, variant at
`crates/holon-api/src/inline_mark.rs:50-70`), `derive_block_links` skips
that variant so no junction row is ever created (`inline_mark.rs:362-368`),
and the GPUI renderer decorates the STORED variant instead of re-classifying
(`frontends/gpui/src/render/builders/text.rs:242-249`). Not self-correcting
across restarts: the cold-boot fast path skips any file whose
`sha256(RENDERER_VERSION ‖ consolidator ‖ disk_bytes)` is unchanged
(`crates/holon-filesystem/src/file_sync_controller.rs:845-855, 1942-1966`)
and the bytes never change, so the poisoned marks are permanent. Violates
Model.md invariant 3 (projection reproducible from the replica: it encodes
the registry contents at the ingest INSTANT, a hidden mutable input) and
invariant 4 (derived holders recompute at quiescence). Found by agent
exploration of the F2a provider lane, not by any test.

## Missing piece

No transition in `E2ETransition`
(`crates/holon-integration-tests/src/pbt/transitions/mod.rs:220-296`) can
register an entity type at runtime — the only registration paths are the
pre-boot DI hook
(`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:620-639`)
and hand-written test code — so the keystone structurally cannot GENERATE a
registration-after-ingest interleaving. The nearest tests all encode the
passing order:
`crates/holon-integration-tests/tests/boot_projector_gated_on_scan.rs` gates
the projector on the scan and says nothing about entity registration, and
`structural_pbt.rs::sidecar_entity_link_resolves_through_the_intent_boundary`
registers BEFORE the link is authored. Not ORACLE: the existing junction
oracle (`structural_pbt.rs:1619-1665`, prod `derive_block_links` vs the
`block_links` rows) WOULD have gone red had the interleaving been generated.
Missing piece = a `RegisterEntityScheme` transition (driven through the
`create_entity_type` MCP tool) plus a scheme-shaped link arm in
`typing_text_strategy`, which today mints only bare wiki-name links
(`crates/holon-integration-tests/src/pbt/generators.rs:236-253`).

## Remedy

OPEN 2026-08-01 — ruling D (option D-min) ratified by Martin; plan in
`PLAN-98D.md`. Fix removes the stored verdict rather than ordering the boot:
`EntityRef::Internal` + `EntityRef::UnknownScheme` merge into a neutral
`EntityRef::Scheme { uri }` (serde/Loro aliases keep both old wire tags
loadable), `derive_block_links` emits the junction row unconditionally, and
Healthy/Unresolved is decided LIVE by the classifier at render. Options A
(boot-ordering gate) and C (registry-aware projection hash) explicitly
REJECTED — C is unsound without A (permanent re-ingest ping-pong). No
registration-triggered sweep is needed: a consumer audit found the only
route from `block_links` back to org bytes is `kind='page'`-filtered
(`crates/holon-app/src/turso_seams.rs:216`), and the `backlinks` matview
joins the SOURCE block only, so junction rows for a not-yet-registered
scheme are inert and self-healing. Red-for-the-right-reason to be captured
by `structural_pbt.rs::entity_scheme_registered_after_org_scan_still_links`
(predicted: empty `block_links` result set) before the fix lands.
