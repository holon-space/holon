# GitHub MCP integration — handoff (2026-05-14)

## Where we are

Plan reviewed and rewritten at `~/.claude/plans/sleepy-booping-prism.md`. Phase 2 (the structural code work in `mcp_vtable.rs`) is **landed and green**. Phase 0 (live MCP probe) and Phase 3 (YAML) are **not started** — they need a live token + endpoint decision.

Working tree status: changes are uncommitted in `crates/holon-mcp-client/src/mcp_vtable.rs` only. No other files touched in this session beyond the plan + this devlog. 61/61 `holon-mcp-client` tests pass. Workspace build clean (only pre-existing dead_code warnings in unrelated files).

## What landed (Phase 2)

All in `crates/holon-mcp-client/src/mcp_vtable.rs`:

- **`EnumerateFrom`** supports two shapes via untagged-style `Option`s — legacy `field: id` (back-compat for `claude-history.yaml`) and new paired `fields: { tool_param: parent_column, ... }`. `bindings(owning_param)` helper unifies both into `Vec<(parent_column, tool_param)>`.
- **`FilterColumnConfig`** gained `enumerate_from: Option<EnumerateFrom>` (serde-default, additive).
- **`ResolvedEnumeration`** now carries `bindings: Vec<(String, String)>` alongside the pre-built SQL. `from_enumerate_from(ef, owning_param, prefix)` builder.
- **`FetchMode::Tool`** gained `enumerations: HashMap<String, ResolvedEnumeration>` keyed by owning tool param name.
- **`McpCursor::filter`** — Tool branch now does FK fan-out symmetric with URI branch. Both share `pick_unresolved_enumerations_{tool,uri}` + `run_enumeration` helpers. Multiple independent unresolved enumerations are rejected by `assert!(unresolved.len() <= 1, ...)` — correlated FKs must use paired `fields:`.
- **`build_fdw_metadata`** — any param bound by some enumeration (own or paired) is forced non-required at the `KeyColumn` level, regardless of YAML `required: true`.
- **`run_enumeration`** (renamed from `enumerate_enumeration_values`) — returns `Vec<HashMap<String, String>>`. Fails loud on NULL/BLOB FK columns (was silently skipping with `_ => {}` before).
- **Rename `fallback` → `enumeration` throughout** — archlint flags `fallback` as a word smell, and the concept genuinely is "enumeration source," not a failure-mode fallback. `ResolvedFallback` → `ResolvedEnumeration`. ~25 occurrences renamed; no functional change beyond identifier names.
- **8 new tests** at the bottom of the test module:
  - `enumerate_from_legacy_single_field_parses`
  - `enumerate_from_paired_fields_parses`
  - `resolved_enumeration_sql_for_paired_fields`
  - `filter_mapping_with_paired_enumerate_from_parses`
  - `paired_enumeration_marks_both_params_non_required`
  - `tool_fetch_mode_carries_enumerations`
  - `pick_unresolved_tool_returns_single`
  - `pick_unresolved_tool_rejects_multiple_independent` (`#[should_panic]`)

## What's NOT done

### Phase 0 — live MCP probe (BLOCKING for everything else)

The plan explicitly gates Phase 3+ on a probe record at `~/.claude/plans/sleepy-booping-prism-probe.md`. Open questions you need to answer with a live token:

| Hypothesis | Verify how |
|---|---|
| H1 — endpoint + auth: hosted `api.githubcopilot.com/mcp/` works with a PAT, vs. needs Copilot OAuth, vs. self-host `github-mcp-server` Docker | Use the `mcp-explorer` skill against the chosen endpoint with a PAT. Raw `curl` won't work — MCP HTTP requires `initialize` → `tools/list` and uses streaming/SSE headers |
| H2 — tool names: `search_repositories` / `list_issues` / `list_pull_requests` (or different) | `tools/list` after `initialize` |
| H3 — response envelope keys: `repositories` / `issues` / `pull_requests` (or different) | One `tools/call` per tool, inspect the top-level JSON key |
| Pagination contract | Cursor token? `page`/`per_page`? Max page size? |
| Repo discovery: `search_repositories` is capped at 100 + public-owned-only — almost certainly want `list_repositories_for_authenticated_user` instead | confirm in `tools/list` |

### Phase 1 — schema mapper validation

Two reads, ~30 min, blocking Phase 3 YAML correctness:

- `crates/holon-mcp-client/src/mcp_schema_mapping.rs` — does it handle nested JSON paths automatically (`owner.login` → flat `owner` column)? If not, the YAML needs a `source_path` field on schema columns (small extension) OR the schema must use `full_name` only and parse client-side.
- Same file: how do arrays land in `TEXT` columns for `labels`/`assignees`? JSON-encoded? Rejected? Comma-joined?

### Phase 3 — GitHub YAML

Template is in the plan. Drop into `~/.config/holon/integrations/github.yaml` after Phase 0 fills in tool names + `extract_path`. Remember `chmod 600`.

### Tests we deferred

The plan's Phase 2.8 (option a) called for a `MockPeer` harness to runtime-test fan-out end-to-end. **Not built.** Current tests are config-parsing + helper-level (`pick_unresolved_*`). The runtime fan-out path (`McpCursor::filter` → `run_enumeration` → `call_tool_with_params` loop) is only exercised live. Budget ~2–4h for a `MockPeer` if you want a regression gate before the live YAML lands.

## Quick start for next session

```bash
# 1. Verify the Phase 2 code is still green
cargo nextest run -p holon-mcp-client

# 2. Phase 0: probe the live MCP
#    Pick endpoint, get a token, then via Claude Code:
/mcp-explorer
#    Record findings in ~/.claude/plans/sleepy-booping-prism-probe.md

# 3. Phase 1: read the schema mapper
ast-outline outline crates/holon-mcp-client/src/mcp_schema_mapping.rs

# 4. Phase 3: copy the YAML template from the plan, fill in probe results
$EDITOR ~/.config/holon/integrations/github.yaml
chmod 600 ~/.config/holon/integrations/github.yaml

# 5. Launch a frontend, verify load:
#    log should show: [load_integration_configs] Loaded provider 'github' …
#    via the `holon` MCP server: list_tables, SELECT count(*) FROM gh_repository, etc.
```

## Critical traps to remember

- **`write_through: true`** is read-side cache population for IVM, NOT remote writes. Safe for read-only entities. (`mcp_integration.rs:483-490`)
- **archlint's `fallback` word smell** trips on edits. We renamed everywhere in `mcp_vtable.rs`; don't reintroduce. The lint message points to `// ALLOW(fallback): <reason>` but rename is cleaner.
- **Paired bindings are explicit** — `fields: { tool_param: parent_column }`. Direction matters: keys are tool params, values are parent columns. Don't write a positional list.
- **One enumeration per fetch only** — multi-independent unresolved enumerations panic (Cartesian product is almost always wrong for FKs). Correlated FKs use paired `fields:`.
- **Loader is dir-driven** — any new `*.yaml` in `~/.config/holon/integrations/` is auto-discovered. No code change needed to register `github.yaml`. Verified by grep, not just claimed.
- **`api.githubcopilot.com/mcp/` likely needs Copilot OAuth, not a raw PAT.** Plan's previous draft assumed PAT — likely wrong. Probe before writing YAML.

## Files touched this session

- `crates/holon-mcp-client/src/mcp_vtable.rs` (M) — Phase 2 implementation
- `~/.claude/plans/sleepy-booping-prism.md` (rewrite) — plan after senior review
- `devlog/2026-05-14-github-mcp-handoff.md` (this file)

No other crates touched. No commits made.
