---
date: "2026-06-19"
session: "f3425fce-0111-4d09-9074-13c71bc7cc0d"
project: "holon"
---

## E3: delete `SutOrgRender` off `E2ESut` (blocker slice removed)

**Context:** Bundle E / E3 (`docs/Testing/PbtCompositionBacklog.md`) dissolves the
monolithic `E2ESut` god-type by deleting its headless SUT-capability impls once each
cap's coverage lives in a composed slice. `SutWatchRows` and `SutOrgRead` were deleted
in earlier increments; `SutOrgRender` was **deferred** for one reason — the standalone
regression slice `tests/org_render_fixed_point_pbt.rs` drove `inv-org-render-fixed-point`
against the full `E2ESut` pipeline, so the cap was not dead code.

**What happened:**
- The user deleted `tests/org_render_fixed_point_pbt.rs` (and `loro_backend_pbt.rs`)
  this session, removing the one blocker. A read-only audit confirmed no remaining
  consumers of `SutOrgRender` / `InvOrgRenderFixedPoint` outside `E2ESut`
  (`org_roundtrip_pbt.rs` only mentions the id in prose and drives `OrgRenderer`
  directly — not a consumer).
- Applied the established 4-edit deletion recipe in `invariant_runner.rs`: dropped
  `SutOrgRender` from the `WideProxyCaps` supertrait + blanket impl, removed
  `InvOrgRenderFixedPoint` from `native_proxy_invariants()` + its `use`, and added
  `inv-org-render-fixed-point` to `NATIVE_ONLY_EXCLUDED` (splitting the entangled
  `SutOrgRead` comment).
- Deleted `impl SutOrgRender for E2ESut` (`sut_capabilities.rs`) → tombstone comment,
  matching the `SutOrgRead`/`SutWatchRows` convention.
- Cleanup: removed the now-dead `TestContext::snapshot_org_render_pairs` helper
  (`test_environment.rs`; the `frontend_slice` component has its own impl) and the
  orphaned `tests/fixtures/org_render_fixed_point_pbt/` dir.

**Key decisions:**
- **Coverage is preserved with teeth, not just catalog-covered.** The composed
  `frontend_slice` is now the sole host: `frontend_slice_org_render_fixed_point_bites`
  drives `inv-org-render-fixed-point` to both arms over the production
  `CacheBlockReader` + `OrgRenderer` (clean → `Ok`, overwrite garbage → `Fail`), and the
  parity gate `composed_catalog_covers_e1_relocated_caps` asserts the composed catalog
  still covers the id. The native selection path already excluded the id anyway
  (`registry.rs:984` — its `min_sut` carries `TursoProjection`), so this was a pure
  E2ESut-impl removal.
- **Disclosed seed retirement.** The fixture dir held `wide_pbt_seed_2026-05-19.json`,
  a wide-PBT regression seed parked under the deleted slice. No `.rs` referenced it; it
  was knowingly retired with the dir. The deleted slice's exact `#+TODO:`-header-drop
  disk-mutation scenario is no longer a dedicated reproducer — re-introduce it as a
  composed `StateMachineTest` step if that loop class recurs.

**Status — all gates green:**
- Compile: `cargo test -p holon-integration-tests -p holon-gpui --features pbt --no-run`
  → exit 0 (all binaries, headless + windowed).
- Composed lib suite: 124 passed / 2 failed — the two pre-existing reds
  (`every_body_file_has_a_registry_entry`, `now_query_compiles_to_canonical_sql`); the
  balance oracle `native_runner_dispatches_exactly_the_registry`, the parity gate, and
  the `memory_slice` selection guard all pass.
- Windowed `gpui_window_slice`: 1/1.

**Remaining E3:** the bulk caps (`SutBackend` / `SutSqlProjection` / `SutViewModel` /
`SutRenderer` / `SutLoroLog` / `SutErrorLog`) stay in `WideProxyCaps` — consumed by many
native + standalone slices; removal is gated on that coverage moving into composed
slices. `SutEditorMirrorWrite` + the editor `_self_` invariants are E5.
