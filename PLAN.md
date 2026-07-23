# Dense org query/patch MCP tools — implementation plan (Increment 0 audit)

Ruling authority: Martin (2026-07-23). Representation: **org container, NOT TOON**.
Dense form = org text with the `:PROPERTIES:/:ID:/:END:` scaffolding compressed to
a trailing headline token `{#<alias>}` (projection only; on-disk org keeps drawers),
plus short per-query base62 ID aliases (bare UUIDs are ~45% of projection tokens).

Bookmark: `dense-org-mcp`, based on `main` (a7534461). NOT on the toon-experiment stack.

## Reuse vs build decisions

### REUSE (audited, do not rebuild)
- **Org render**: `Block::to_org` / `OrgRenderer::render_document`+`render_entitys`
  (`crates/holon-org-format/src/{models.rs:811, org_renderer.rs:27,62}`). Dense render
  parameterizes the identity emission point only (drawer `:ID:` line ⇄ trailing token).
- **Org parse**: `parse_org_file(path, content, parent_dir_id, root) -> ParseResult`
  (`crates/holon-org-format/src/parser.rs:120`). Dense parse calls it, then strips the
  trailing `{#token}` from each headline's title at the boundary. ONE parser.
- **Block model + accessors**: `Block` (`holon-api/src/block.rs:287`), `OrgBlockExt`
  (`org_title`, `task_state`, `level`, `get_block_id`, `org_properties`) in
  `holon-org-format/src/models.rs`. Task state lives in `properties["task_state"]` +
  `["task_state_category"]` ("active"/"done").
- **Query engine**: `HolonService::execute_query(query, QueryLanguage, params, context)`
  via MCP `execute_query`. Dense query tool is a thin PRQL/SQL preset (subtree-of,
  exclude-state-category, depth) + dense renderer. `task_state_category` column exists.
- **Write path**: `HolonService::execute_operation(EntityName("block"), op, StorageEntity)`.
  Op vocabulary (audited): `create`, `update`, `set_field(id,field,value)`, `delete(id)`,
  `move_block(id,parent_id,after_block_id?)`, `cycle_task_state`. `StorageEntity =
  HashMap<Arc<str>, holon_api::Value>`.
- **Read/snapshot**: `HolonService::execute_raw_sql("SELECT ... FROM block_raw WHERE ...")`
  → rows of `StorageEntity`. `write_seq` + `updated_at` columns = concurrency token.
- **MCP tool pattern**: `#[tool_router]` / `#[tool(description=...)]` on `impl
  HolonMcpServer`, `Parameters<T>` params, `Result<CallToolResult, rmcp::ErrorData>`.
  `self.service()` → HolonService with `OpOrigin::Agent`. `get_loro_blocks(doc_id)`,
  `resolve_to_file_path`, `build_context` helpers exist (`frontends/mcp/src/tools.rs`).
- **PBT harness**: `compose_sut(set, resolver)` headless engine+store
  (`holon-integration-tests/src/pbt/composed/builder.rs`), `E2ETransition` alphabet,
  `ReferenceState` oracle, dedicated-PBT-as-one-`ComposedSlice`-impl pattern
  (`frontend_slice/structural_pbt.rs`). Feature gate `pbt`
  (`holon-integration-tests/Cargo.toml`). Normalized block compare:
  `holon-block-roundtrip-testing::assert_normalized_docs_equal`.

### BUILD (new)
- `crates/holon-org-format/src/dense.rs` — the ONE dense syntax home:
  - `Alias` newtype (base62, validated) + `AliasTable` (bidi id⇄alias, deterministic
    shortest-fit assignment by projection order).
  - `render_dense(blocks, file_id, &AliasTable) -> String` (drawer ID → `{#alias}` token;
    non-ID drawer content preserved; requires-targets aliased when in table).
  - `parse_dense(text) -> Result<DenseParse>` reusing `parse_org_file` + trailing-token
    boundary strip. `DenseBlock { alias: Option<Alias>, level, title, task_state, requires,
    body }` (alias None ⇒ NEW block).
  - `plan_patch(snapshot, &DenseParse, &AliasTable, deletes) -> Result<PatchPlan>` pure
    planner: diff parsed tree vs projection snapshot → typed `PatchOp` list + the
    conflict-check set. No engine, no async — unit/PBT-testable.
- `frontends/mcp/src/dense_projection.rs` — `ProjectionRegistry` (handle → snapshot +
  alias table + per-block concurrency token, TTL), and the engine-side applier
  (concurrency re-read → `execute_operation`). Structured conflict/stale errors.
- Two MCP tools in `tools.rs`: `dense_query` (project) and `dense_patch` (apply).

## Concurrency contract
Per-block token = `(write_seq, updated_at)` captured at projection. No built-in CAS
(Phase-2 removed `_expected_*` guards), so the patch tool re-reads each touched block
and rejects with a structured conflict listing changed blocks BEFORE applying any op.
Honors the EBO dirty-editor policy: a block edited between project and patch fails loud,
never clobbered. Deletion only via explicit `delete: [aliases]` param (never by omission).

## Increments (one commit each, bookmark `dense-org-mcp`)
1. `dense.rs` render + `{#token}` parse + round-trip PBT (red first).
2. `dense_query` tool end-to-end (projection + handle + aliases) + `ProjectionRegistry`.
3. `dense_patch` tool + planner + concurrency + keystone-shaped PBT (red first):
   project → mutate → patch → store == same edits via direct reference ops. Plus
   conflict-rejection, stale-handle, alias round-trip coverage.
4. Wire into MCP registry; agent-grade tool descriptions.

## Privacy
Repo is PUBLIC. Synthetic data only in all committed files. Grep diff for `holon-pkm`,
`Users/martin`, and any vault-copied string before every commit.
