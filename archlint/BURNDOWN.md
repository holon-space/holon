# archlint baseline burn-down

`archlint/baseline.txt` grandfathers **154 pre-existing architecture violations**
that were invisible until PR #100 fixed the `analyze-arch` recipe's missing
`set -o pipefail` (the recipe used to exit `tee`'s status = always 0, false-green).

`./archlint/archlint --all` now **fails only on NEW violations** (any hit not in the
baseline). Every run still discloses the baselined count, so the tree never reads as
debt-free:

```
archlint: 154 baselined violation(s) suppressed (see archlint/baseline.txt), 0 new violation(s).
```

## Ratchet rules

- **Never grow the baseline.** New violations fail the gate — fix them, don't add to
  `baseline.txt`. `--update-baseline` is for ratcheting DOWN only.
- After fixing a baselined violation, run `./archlint/archlint --update-baseline` to
  drop it. A run that leaves entries no longer firing prints a **stale** nag:
  `archlint: baseline stale - K entry(ies) no longer fire; run ./archlint/archlint --update-baseline to ratchet down.`
- Identity = `rule_id` + repo-relative path + whitespace-collapsed source line
  (NOT line number → survives line drift). Duplicates tracked by multiplicity.

## Snapshot (2026-07-27, integration tip 8d7209e0 + HANDOFF move)

| Rule | Count | Class |
|---|---:|---|
| `entity_uri_from_raw` | 65 | mechanical — mostly genuine boundaries needing disclosed `// ALLOW(entity_uri_from_raw): <source>` |
| `no-underscore-params` | 49 | mechanical — bare `_` / remove / `ALLOW(unused_param)` for trait sigs |
| `fallback` | 15 | mechanical — justify with `ALLOW(fallback)` or remove the smell |
| `ok` | 8 | mechanical — `?` / `.context` / `.expect`, or `ALLOW(ok)` |
| `raw_sql_in_frontend` | 7 | **structural** |
| `frontend-raw-sql` | 3 | **structural** |
| `deleted_cell_symbol` | 2 | **structural** |
| `sole_block_writer` | 1 | **structural** |
| `mcp-client-holon-dep` | 1 | **structural** |
| `frontend-storage-backend` | 1 | **structural** |
| `frontend-provider-dep` | 1 | **structural** |
| `entity_uri_parse_default` | 1 | **structural** |
| **Total** | **154** | |

## Priority: the 17 structural violations (fix these first, individually)

These carry real architectural meaning (crate decoupling, single-writer invariant,
storage-agnostic frontend layer). Burn down before the 137 mechanical/annotation debt.

1. `sole_block_writer` (1) — `crates/holon/src/storage/turso_sink_reader.rs:127`
   Raw block-table write outside the sanctioned single writer → dual-writer divergence
   bug class (docs/Architecture/Replication.md §5/§9). Route via BlockOperations, or
   `ALLOW(sole_block_writer)` with a disclosed reason. **Highest risk.**
2. `frontend-storage-backend` (1) — `crates/holon-frontend/src/rich_text_selection.rs:12`
   holon-frontend must stay storage-agnostic (no Loro/Turso). Route via capability traits.
3. `frontend-provider-dep` (1) — `frontends/gpui/Cargo.toml:103`
   Frontend depends on a provider crate directly; wire via DI instead.
4. `mcp-client-holon-dep` (1) — `crates/holon-mcp-client/src/write_authorization.rs:139`
   holon-mcp-client imports the `holon` engine crate (Rev 3.5b decoupling). Use holon-core seams.
5. `frontend-raw-sql` (3) — `crates/holon-frontend/src/lib.rs:120`,
   `crates/holon-frontend/src/advice_weaver.rs:86`, `:106`
   Raw SQL in the storage-agnostic ViewModel layer. Move behind QueryEngine / BlockQuerySource.
6. `raw_sql_in_frontend` (7):
   - `frontends/gpui/src/tab_strip.rs:88` (`OPEN_TABS_SQL`), `:92` (`CURSOR_SQL`) — render-data
     reads; move behind a `BackendEngine` method.
   - `frontends/gpui/src/oracles_ui.rs:65`, `:92` — debug-oracle read path; move to backend or
     `ALLOW(sql)` (oracle is a diagnostic surface).
   - `frontends/holon-worker/src/seed.rs:131` — config comment explicitly names seed.rs as an
     accepted seeding path → `ALLOW(sql)` with that justification.
   - `frontends/holon-worker/src/lib.rs:1452` — worker `execute_raw_sql` SELECT; expose via a
     service method or `ALLOW(sql)`.
   - `frontends/dioxus-web/src/main.rs:180` — bootstrap existence check; expose via BackendEngine.
7. `deleted_cell_symbol` (2) — `crates/holon-api/src/block_write_field.rs:191`, `:193`
   Pre-Phase-2 watermark symbol (`_expected_content`) reappeared. Rename/remove or `ALLOW`.
8. `entity_uri_parse_default` (1) — `frontends/gpui/src/render/builders/live_block.rs:20`
   `EntityUri::parse(..).unwrap_or*` substitutes a default URI. Use `.expect` (internal bug =
   fail loud) or propagate the `Result`.

## Then: the 137 mechanical/annotation debt

`entity_uri_from_raw` (65), `no-underscore-params` (49), `fallback` (15), `ok` (8). Most
`entity_uri_from_raw` hits are genuine boundaries (org parser, SQL rows, Loro fields, test
literals) that just need a disclosed `// ALLOW(entity_uri_from_raw): <names the source>`;
each `ALLOW` must carry a real per-site justification, so this is bulk-but-not-blind work.
