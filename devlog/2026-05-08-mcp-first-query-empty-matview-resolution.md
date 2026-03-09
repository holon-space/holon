# Resolution: MCP `now-query` returns 0 rows on first call

**Status:** FIXED upstream; holon-side warmup is unnecessary and was reverted.
**Supersedes:** `devlog/2026-05-08-014627-handoff-mcp-first-query-empty-matview.md`.

## Outcome

- **Hypothesis confirmed:** Turso `MaterializedViewCursor` first-open
  laziness on autocommit reads. The first SELECT against a freshly-built
  matview returned a partial subset; subsequent SELECTs returned the
  full result.
- **Pure-Turso reproducers landed**:
  - Public-API standalone:
    `bigdata/turso/bindings/rust/examples/matview_first_open_partial.rs`
    (registered in `bindings/rust/Cargo.toml`).
  - Holon-stack variant (using `turso_sdk_kit::rsapi::TursoConnection`
    the way holon does in production):
    `crates/holon/examples/turso_ivm_matview_first_open_empty_repro.rs`.
  - SQL trace: `bigdata/turso/bugs/holon_block_matview_first_open_empty_2026-05-08.{sql,md}`.
- **Upstream fix landed**: nightscape@holon `290fbb4ff` —
  *"fix: IVM matview cursor returns partial result on first read after
  IO yield"*. Holon's `Cargo.lock` is now pinned at that commit.
- **All four scenarios** in both reproducers now show `1000 → 1000`
  consistent counts.
- **End-to-end verified**: a fresh `holon-mcp` start with the fixed
  Turso pin returns `968` (the full count) on the very first
  `SELECT COUNT(*) FROM block` MCP call — no warmup, no
  `ensure_warmed` plumbing, no `wait_quiescent` gate.
- **Reverted**: the entire workaround stack (`OrgSyncIdleSignal::wait_quiescent`,
  `HolonMcpServer::ensure_warmed`, `with_type_registry_and_idle`, the
  `SELECT COUNT(*) FROM block` warmup in `main.rs`) — none of it is
  needed against the fixed Turso.

## Investigation insight worth keeping

`SELECT 1 FROM block LIMIT 1` did **not** work as a warmup in
production: the limit short-circuited the cursor before its DBSP
incremental state was fully realised, so the user's later `COUNT(*)`
still hit the cold path. `SELECT COUNT(*) FROM block` did work because
it walked every row.

Generalisation: when this class of bug recurs upstream (different
cursor flavour, different yield boundary), pick a warmup that
exercises the same code path the user's query will exercise — full
scans are safe; `LIMIT 1` is not.

## Reproducers retained as regression gates

The two reproducers should pass forever now. If a future Turso pin
regresses this, both `cargo run --example matview_first_open_partial -p turso`
(in the Turso checkout) and
`cargo run --example turso_ivm_matview_first_open_empty_repro` (in the
holon checkout) will surface the regression with a single
`*** BUG: first SELECT returned ...` line.
