# Crate Structure & Coupling Review (Fable, 2026-07-06)

Read-only architecture review of the Holon workspace: 23 crates under `crates/`,
5+ frontends, hakari `workspace-hack`, archlint gates. Evidence gathered via
Cargo.toml inspection, `cargo tree -e normal -i`, `ast-outline cycles`, and the
archlint rule set (`archlint/smells/imports.toml`, `docs/Architecture/Archlint.md`).

Verdict up front: the *intended* architecture (Ports in holon-core, Shared Kernel
in holon-api, composition in holon-app, archlint as the cop) is genuinely good and
mostly enforced — for `holon-orgmode`, `holon-frontend`, and `holon-mcp-client`.
The three biggest problems are (1) a hakari feature-unification accident that
compiles the entire GPUI framework + proptest into every native production build,
(2) the `holon` facade crate being a second, un-linted composition root that names
both concrete backends, and (3) `frontends/mcp` being a de-facto library crate that
inverts the crates→frontends layering.

---

## P0 — production-build contamination

### P0.1 workspace-hack drags `gpui` (with `test-support` → proptest) into EVERY native prod build

**Evidence:**
- `workspace-hack/Cargo.toml:48` and `:131`:
  `gpui = { git = "https://github.com/holon-space/zed.git", branch = "holon", features = ["test-support"] }`
  — in the *normal* `[target.'cfg(not(wasm32))'.dependencies]` table.
- `cargo tree -p holon-tui -e normal -i proptest` proves the chain:
  `proptest ← gpui v0.2.2 ← workspace-hack ← {holon, holon-api, holon-core, holon-turso, holon-loro, holon-orgmode, holon-mcp, holon-tui, …}`.
- Root cause: `frontends/gpui/Cargo.toml:150` — a **dev**-dependency
  `gpui = { …, features = ["test-support"] }` — which cargo-hakari unifies into the
  workspace-hack's *normal* dependency set (default hakari traversal includes
  dev-deps of workspace members). `.config/hakari.toml` has no
  `[traversal-excludes]` / `[final-excludes]`.

**Why it's a problem:** the MCP server, the TUI, `holon` itself, and every adapter
crate now *compile the full GPUI GUI framework* (wgpu/metal stack) plus `proptest`
and `proptest-macro` as normal dependencies. This is exactly the "proptest in prod
builds" smell flagged in the 2026-07-02 crate-slicing audit — root cause is hakari,
not any crate's own manifest (the crates' own gating via `testing`/`test-helpers`
features is correct). It bloats clean-build time for every non-GUI target, bloats
the MCP/TUI binaries' dependency surface, and silently defeats the carefully
feature-gated `holon-frontend/pbt`, `holon/testing` discipline.

**Fix (concrete):** add to `.config/hakari.toml`:
```toml
[final-excludes]
third-party = [
    { name = "gpui", git = "https://github.com/holon-space/zed.git" },
]
```
then `cargo hakari generate` (re-apply the wasm32 table-header gate noted in the
workspace-hack file comment), and verify with
`cargo tree -p holon-tui -e normal | grep -c 'gpui v\|proptest'` → 0.
Optionally also drop `test-support` from `frontends/gpui/Cargo.toml:150` if the
dev tests don't actually use it. Add a CI assert (a 5-line test in
`holon-architecture-tests`) that `cargo tree -p holon-mcp -e normal` contains
neither `gpui` nor `proptest` so this can never regress silently.

### P0.2 `[patch]` points at an absolute local path — build is not reproducible

**Evidence:** root `Cargo.toml:265-266`:
```toml
[patch."https://github.com/holon-space/gpui-component"]
gpui-component = { path = "/Users/martin/Workspaces/rust/gpui-component/crates/ui" }
```
**Why:** anyone (CI, another machine, a worktree agent fleet) without that exact
path gets a hard resolution failure or silently different code. The comment admits
it ("once pushed … can be replaced").
**Fix:** push the `context_menu_extender` commit to `holon-space/gpui-component`
and restore the git+branch patch. This is a one-commit fix; do it before the next
multi-agent session — worktree agents share this manifest.

---

## P1 — layering claims that are false in the dependency graph

### P1.1 `holon` (Core / "Facade") is a second composition root that names both concrete backends

**Evidence:**
- `crates/holon/Cargo.toml` `[dependencies]`: `holon-turso`, `holon-loro`,
  `holon-petri`, `holon-engine`, `holon-profiles`.
- Backend-named modules inside the crate:
  `crates/holon/src/sync/loro_module.rs`, `sync/loro_block_query_source.rs`,
  `sync/turso_block_query_source.rs`, `storage/turso_sink_reader.rs`,
  `storage/turso_block_link_indexer.rs`, plus a full `di/` tree
  (`lifecycle.rs`, `registration.rs`, `runtime.rs`, `schema_providers.rs`).
- `docs/Architecture/CrateMap.md` claims: *holon-app "owns **every** wiring that
  names concrete backends"*. `holon-app` is in fact thin (3,020 LOC — good), but
  `holon` (18,571 LOC) contradicts the claim in both manifest and module names.
- archlint enforces "must not name Loro/Turso" only for `holon-orgmode`
  (`smells/imports.toml` ids `loro`/`turso`) and `holon-frontend`
  (`frontend-storage-backend`). `holon` is exempt **by omission**, not by decision.

**Why:** the strongest anti-leak rule in the codebase ("guards must never name
Loro/Turso", Principles.md; the consolidator-epoch memory says the same) has a
hole exactly at the crate labeled "Facade". Everything downstream of `holon`
(holon-app, all frontends) transitively hard-links both backends regardless of
`CapabilityProfile`; the `holon-app/no_turso.rs` seam can't ever be honest at the
link level. It also makes `holon` the god-crate: it is simultaneously facade,
sync pipeline, storage API, DI registry, and test-helper host.

**Fix:** (staged, not a big-bang)
1. Move the five backend-named modules out of `holon`: `loro_module` /
   `loro_block_query_source` → `holon-loro` (or `holon-app::loro_seams`);
   `turso_sink_reader` / `turso_block_link_indexer` / `turso_block_query_source`
   → `holon-turso` (or `holon-app::turso_seams`, where the pattern already exists).
2. Then cut `holon-turso` + `holon-loro` from `holon`'s manifest.
3. Lock it in: add manifest smells to `archlint/smells/imports.toml` mirroring
   `orgmode-holon-dep-manifest`, e.g. `holon-no-backend-dep-manifest` matching
   `^holon-(turso|loro)` in `crates/holon/Cargo.toml`.
4. Update the CrateMap `@c4` blurb for `holon` — today it *advertises* the
   violation ("sync pipeline (Loro, OrgMode, Iroh), storage API").

If step 2 proves too expensive this cycle, do step 3 inverted (a baseline'd
allowlist) so the hole is at least a *disclosed* exemption instead of an omission.

### P1.2 `frontends/mcp` is a library masquerading as a frontend — crates depend upward on it

**Evidence:**
- `crates/holon-integration-tests/Cargo.toml:103`:
  `holon-mcp = { path = "../../frontends/mcp", optional = true }` — a **crates/**
  member depending on a **frontends/** container.
- `frontends/gpui/Cargo.toml` and `frontends/tui/Cargo.toml` both carry
  `holon-mcp = { path = "../mcp" }` as *normal* deps (every GUI binary embeds the
  MCP server — intended behavior per CLAUDE.md, but expressed as a
  frontend→frontend edge).
- `frontends/mcp/Cargo.toml` itself names `holon-loro`, `holon-orgmode
  (features=["di"])`, `holon-filesystem` directly; archlint's
  `frontend-provider-dep` smell explicitly `exclude`s `frontends/mcp/**`
  ("legacy/inline DI").

**Why:** C4-wise `holon-mcp` is documented as a *container* (deployable), but the
graph treats it as a shared component. The exemption list in archlint plus the
upward test-crate edge means the "frontends depend on crates, never the reverse"
rule is unenforceable as stated. It also drags rmcp/axum/tungstenite/image into
gpui and tui builds unconditionally.

**Fix:** split it: `crates/holon-mcp-server` (lib: rmcp service, tool registry,
embedding API) + `frontends/mcp` (thin bin wrapper).
gpui/tui/integration-tests re-point at the lib crate; the archlint mcp exemptions
shrink to the bin. While splitting, route its DI through `holon-app` (it already
depends on holon-app!) instead of inline `holon-orgmode/di` wiring, which would
let you delete the `di` feature of holon-orgmode (see P2.2).

### P1.3 `holon-api` "Shared Kernel" is two kernels in a trenchcoat (data + UI render vocabulary)

**Evidence:** `crates/holon-api/src` = 14,785 LOC / 34 modules. Alongside
storage/CDC/operation types sit `render_dsl.rs` (906), `render_types.rs` (1,016),
`render_eval.rs` (1,523), `widget_spec.rs` (313), `widget_meta.rs`, `ui_watcher.rs`
— ≈3,850 LOC of UI/render vocabulary. Every adapter (`holon-turso`, `holon-loro`,
`holon-markdown`, `holon-org-format`, `holon-mcp-client`, `holon-filesystem`)
depends on `holon-api`.

**Why:** the "No frontend deps" claim holds at the Cargo level (deps are only
`holon-expr` + `holon-macros` — verified), but the *content* makes every storage
adapter recompile when a widget spec or the render DSL changes. Render evaluation
logic (`render_eval.rs`, the largest module) is not "shared value types" — it's
Interpreter-pattern machinery that belongs beside `holon-frontend`'s
`render_interpreter.rs` (which sits in that crate's one real dependency cycle, see
P2.4 — evidence the seam is in the wrong place).

**Fix:** extract `holon-render-spec` (or fold into `holon-frontend` if nothing
below the ViewModel layer truly needs it — audit `ui_watcher`/`widget_meta`
consumers first with `ast-outline reverse-deps`). Adapters keep depending
on a slimmer holon-api. This halves the incremental-rebuild blast radius of the
most-depended-on crate in the workspace.

---

## P2 — asymmetries, drift, and hygiene

### P2.1 Adapter split is incoherent: org = 2 crates, markdown = 1, and the `FileFormatAdapter` impls live at different layers

**Evidence:** `holon-org-format` (pure parse/render/diff) + `holon-orgmode`
(disk I/O + sync + DI) vs `holon-markdown` (parse + render + adapter impl in one).
`impl FileFormatAdapter` lives in `crates/holon-markdown/src/file_format.rs:59`
(the *format* crate) but in `crates/holon-orgmode/src/file_format.rs:32` (the
*sync* crate), not in `holon-org-format`.
**Why:** the next contributor adding a format has two contradictory templates.
The org split is the right one (pure core, effectful shell); markdown just hasn't
earned its second crate yet — fine — but the adapter-impl *home* should match.
**Fix:** move `OrgFormatAdapter` into `holon-org-format` (the impl looks like pure
parse/render work; check its imports first) so the rule is "FileFormatAdapter impl
lives in the format crate; sync/watch machinery lives in the sync crate". Document
the rule in CrateMap's Adapter section.

### P2.2 `holon-orgmode/src/di.rs` (813 LOC, `di` feature) is a third composition root

**Evidence:** `crates/holon-orgmode/src/di.rs` — exempted by name in the
`orgmode-raw-sql` smell ("outside di.rs"); consumed only via
`frontends/mcp` (`features = ["di"]`).
**Why:** three places now do wiring (holon-app, holon/di, orgmode/di). The archlint
message for the `turso` smell says wiring "lives in holon-app::turso_seams" —
di.rs is the sanctioned self-contradiction.
**Fix:** falls out of P1.2 — when frontends/mcp composes via holon-app, delete the
`di` feature and di.rs.

### P2.3 `holon-loro` is storage adapter + network transport; `holon-turso` is storage only

**Evidence:** `crates/holon-loro/src/{iroh_sync_adapter, iroh_advertiser, ticket,
device_key_store, share_peer_id, shared_snapshot_store}.rs`; `iroh-sync` is the
crate's **default** feature and `holon`'s default feature re-enables it.
**Why:** asymmetric with holon-turso and mixes two capabilities (CRDT persistence
vs P2P replication) in one crate; the workspace already fights this ("holon-loro's
default iroh-sync … fatal on wasm" comment in `crates/holon/Cargo.toml`, plus the
ed25519 lock-churn incident was iroh-induced). Default-on also contradicts the
latency finding that CRDT/sync config is the expensive path — desktop default
still links iroh.
**Fix:** extract `holon-iroh-sync` adapter crate (Replication layer), make
`iroh-sync` non-default everywhere, and let `holon-app` compose it per
`CapabilityProfile`. Cheap first step: flip the defaults; the crate split can wait.

### P2.4 File-level cycles (crate graph itself is acyclic — good)

**Evidence (`ast-outline cycles crates`):**
1. `holon-frontend`: 5-file cycle `reactive.rs ↔ reactive_view_model.rs ↔
   render_context.rs ↔ render_interpreter.rs ↔ view_model.rs` — the ViewModel
   core is one tangle; any change touches all five.
2. `holon/src/api/backend_engine.rs ↔ holon/src/di/test_helpers.rs` — prod module
   knows its test helper (cfg-gated at `crates/holon/src/di/mod.rs:15`, but the
   coupling direction is inverted).
3. `holon-loro/src/loro_backend.rs ↔ shared_tree.rs`.
4. `holon-integration-tests` local_caps ↔ toggle_state (test-only, ignore).
**Fix:** for (1), `render_interpreter` should depend on a trait in
`render_context`, not back on `view_model` — worth an hour with
`ast-outline show` before the next ViewModel change; (2) invert: test_helpers
imports backend_engine, never vice versa.

### P2.5 CrateMap silently omits `holon-loro`, `holon-petri`, `holon-profiles`

**Evidence:** no `@c4` annotation in `crates/holon-{loro,petri,profiles}/src/lib.rs`
(grep found none); the generated `docs/Architecture/CrateMap.md` table has no rows
for them. One of the two concrete backends is invisible in the architecture
inventory that `just arch-validate` checks against baseline.
**Fix:** add the three `@c4` annotations; change archidoc/arch-validate to FAIL on
a workspace member without one (an unannotated crate is exactly the one that
escapes review).

### P2.6 `holon-block-roundtrip-testing` → `holon-org-format` (admitted debt)

**Evidence:** `crates/holon-block-roundtrip-testing/Cargo.toml` carries
`holon-org-format` with the in-file comment "moving them to holon-api is a
separate refactor". A "format-and-storage-agnostic" test crate names one format.
**Fix:** do the admitted move (the normalized comparison shapes) or re-title the
crate honestly.

### P2.7 Minor manifest hygiene

- `crates/holon/Cargo.toml`: `holon-engine = { path = "../holon-engine" }` — the
  only internal dep not using `workspace = true`.
- `frontends/gpui/Cargo.toml:82` vs `:150`: normal dep and dev dep of `gpui` are
  declared against `zed-industries/zed` and rely on the root `[patch]`
  (`Cargo.toml:244`) to redirect to the fork — works, but the `test-support`
  divergence between the two declarations is what armed P0.1.

---

## What is actually clean (credit where due)

- **Crate-level graph is acyclic** and the Engine column is exemplary:
  `holon-expr` (no deps) ← `holon-engine` (expr only) ← `holon-petri` /
  `holon-profiles`. Interpreter pattern honored.
- `holon-app` is a genuinely thin composition root (3,020 LOC, seam-named files).
- `holon-orgmode` decoupling (Rev 3.5a) held: no `holon` dep, no direct Loro/Turso,
  and archlint has both source- and manifest-level regression gates. This is the
  template P1.1 should copy.
- `holon-mcp-client → holon-turso` is coupling **by disclosed design** (FDW
  bridge; documented inside imports.toml itself). Acceptable.
- `holon-api` keeps its "no frontend deps" promise at the manifest level;
  `holon-turso`/`holon-core`/`holon-markdown` manifests are minimal and correct.
- Test vocabulary gating (`holon/testing`, `holon-frontend/pbt`,
  `holon-loro/test-helpers`) is done right *per-crate* — it's only hakari (P0.1)
  that defeats it.

## Biggest single win

Fix P0.1 today: exclude `gpui` from workspace-hack and add the
`cargo tree -e normal` regression test. It's a ~10-line change that removes the
entire GPUI framework + proptest from the production dependency closure of the
MCP server, the TUI, and every core/adapter crate — the largest
coupling-per-line-of-fix ratio in this review, and it restores the truth of the
feature-gating discipline the crates already implement correctly.
