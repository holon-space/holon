# Design Review: Petri-net Engine Refactor (merge 30127e8e12)

Reviewer: Fable agent, 2026-07-06. Read-only review of `holon-petri`, `holon-engine`,
`holon-expr`, and the `holon-frontend`/`frontends/*` reactive cleanup.

## 0. Framing correction — what this refactor actually is

The merge commit bundles **two unrelated tracks**, and the review request premise
("the Petri net models the reactive/projection pipeline") is false:

1. **Petri-net WSJF engine** — `holon-expr` (CompiledExpr) → `holon-engine`
   (generic net/marking/guard/rank/what-if, YAML + CLI) → `holon-petri`
   (materializes task `Block`s into a net) → `holon::petri::rank_tasks` →
   MCP `rank_tasks` tool (`frontends/mcp/src/tools.rs:797`). This is a **task-ranking
   simulator**, not a projection system.
2. **Reactive cleanup** — `holon-frontend/src/reactive.rs` (`ReactiveEngine`,
   futures-signals) replaces `AppState` / `BlockWatchRegistry` / `CdcState` /
   `spawn_ui_listener`. The projection/view-model pipeline is *this*, and it never
   touched Petri code.

Both are assessed below, but conflating them in one squash-merge with the message
"complete cleanup" is itself a review finding: the commit narrative made an
unrelated experimental engine look like the replacement for the reactive layer.

## 1. Petri-net fit: right tool, currently under-loaded

The flat-net model (no explicit places; `token_type` acts as an implicit colored
place, lifecycle in a `status` attribute) is a defensible colored-Petri-net
simplification, and WSJF derived as Δobjective/duration via one-step lookahead
(`engine.rs:224-266`) is elegant — no configured priority weights, exactly as the
skill doc promises.

However, in the *only production path* (`rank_tasks`), most of the Petri machinery
is inert:

- Self-executed tasks get an **empty-precondition** input arc on `self`
  (`holon-petri/src/lib.rs:971-981`) — always enabled.
- The generated objective is a pure sum of completion-token weights
  (`lib.rs:1121-1139`); ranking degenerates to `weight/duration` sorting.
- The net only earns its keep where tokens gate enabling: `depends_on` /
  `>`-sequential completion-token preconds (`lib.rs:1060-1077`) and the delegation
  `waiting`-token consume cycle (`lib.rs:984-1035`). Those ARE wired, so the
  abstraction is justified — but the backtracking binder (`engine.rs:83-110`),
  placeholder unification, and constraint machinery are scaffolding waiting for a
  load they do not yet carry.

**Verdict: keep the abstraction; it is not an impedance mismatch for dependency-
aware ranking. But see §3 (dead knobs) — either wire the inert dimensions or cut
them.**

## 2. Strongest design concerns

### C1 — Postconditions and create-arcs are stringly-typed and recompiled per firing

`PrecondSpec` is a textbook parse-do-not-validate type: parsed once at net load into
`Placeholder | Comparison{compiled} | Exact` (`holon-engine/src/arc.rs:52-107`).
Its output-side twins got none of that:

- `OutputArc.postcond: BTreeMap<String, String>` (`arc.rs:171-175`)
- `CreateArc { id_expr: String, attrs: BTreeMap<String, String> }` (`arc.rs:178-183`)
- `eval_postcond` calls `engine.eval_with_scope(expr: &str)` — a full Rhai
  **parse+eval on every firing** (`guard.rs:180-206`), and `rank()` fires every
  enabled transition on a cloned marking (`engine.rs:224-266`), so every rank pass
  re-parses every postcond of every enabled transition. Malformed postconds are
  only discovered at fire time, not load time — a fail-late seam.

**Fix:** introduce `enum PostcondExpr { Placeholder(String), Expr(CompiledExpr) }`
mirroring `PrecondSpec`, parsed in `YamlNet::new` / `build_task_transitions`. This
removes the per-fire parse cost and moves postcond errors to the load boundary.

### C2 — holon-petri builds Rhai source by string-formatting user data (injection)

`build_task_transitions` (`holon-petri/src/lib.rs:959-1119`) constructs create-arc
"expressions" by embedding user-derived strings inside Rhai string literals:

```rust
a.insert("source_task".to_string(), format!("\"{}\"", task.block_id));
a.insert("delegate".to_string(),   format!("\"{person}\""));
```

`person` comes from the `@[[Person]]:` content prefix — **user-typed text**. A name
containing `"` or `\` produces invalid (or semantically different) Rhai and fails at
fire time. `build_objective_expr` (`lib.rs:1121-1139`) likewise splices `bid` into a
Rhai string literal and `{weight:.6}` as a formatted float. This is Rhai injection
by construction, and it also forces per-fire parsing (C1) of what are actually
*constants*.

**Fix:** let `CreateArc.attrs` carry `enum AttrInit { Literal(Value), Expr(CompiledExpr) }`.
holon-petri then passes `Value::String(task.block_id.clone())` directly — no Rhai
source is ever assembled from user data. The objective should reference values via
scope variables, never inline literals.

### C3 — Domain tokens are loose maps + magic strings; illegal states representable

The engine generic traits (`TokenState`/`Marking`/`NetDef`, holon-engine
`lib.rs:23-57`) are the right seam. But holon-petri instantiates them with
`token_type: String` — `"person"`, `"completion"`, `"waiting"`, `"document"`,
`"knowledge"` scattered as literals across five builder functions
(`holon-petri/src/lib.rs:853-955, 959-1119`) — and attributes keyed by magic
strings (`"source_task"`, `"status"`, `"name"`). A typo in any literal produces a
silently never-matching arc, the exact bug class CLAUDE.md parse-do-not-validate
rule targets. Also `TaskMarking.tokens: Vec<TaskToken>` gives O(n) `token()` /
`set_attr()` scans (`lib.rs:169-201`) and O(n²) behavior inside `fire`.

**Fix:** a domain-side `enum HolonTokenType` with `as_str()` at the trait boundary,
typed constructors (`TaskToken::completion(block_id)`), and
`BTreeMap<TokenId, TaskToken>` for the marking. The generic engine can stay
string-typed — YAML nets are inherently open-world — but the *domain adapter* is
exactly where the closed set should be an enum.

### C4 — Dead economic knobs: energy/focus/mental-slots/discount/constraints

- The self token carries `energy`, `focus`, `mental_slots_{occupied,capacity}`
  (`lib.rs:853-880`), but **no materialized transition has a precondition on them
  and the generated objective never references them**. Mental slots surface only as
  display info in `RankResult`. The energy economy is aspirational cargo.
- `TaskNet.constraints` is always `vec![]` (`lib.rs:806`).
- Discounting is broken at both layers: `objective::evaluate` pushes a **constant**
  `discount = 1/(1+rate)` regardless of the marking clock
  (`holon-engine/src/objective.rs:16-21`) — no time dependence, so it cannot
  distinguish "value now" from "value after 8h of sleep"; and the holon-petri
  objective never references `discount` at all, making `discount_rate` a dead
  prototype property in the production path.

This contradicts the skill doc own creed ("If something matters, it is because the
objective function values it") and the project rule against leaving inert
machinery. **Fix:** either (a) wire them — `mental_slots_occupied < capacity`
precond on the self arc, energy cost postconds, time-decayed discount in
`evaluate` — or (b) delete energy/focus/discount from materialization until a
scenario pays for them. Half-wired is the worst state.

### C5 — String errors + library panics on user data

Every fallible engine/petri API returns `Result<_, String>`; `thiserror = "2"` sits
unused in `holon-engine/Cargo.toml`. Worse, holon-petri **panics** on bad stored
data: `numeric_prop`/`integer_prop` (`lib.rs:467-488`), `resolve_prototype` on Rhai
eval error (`lib.rs:346-360`), `block_to_prototype_props`, and the
`rhai_ident_fragment` collision assert in `materialize_at` (`lib.rs:751-761` — and
the collision is real: `a-b` and `a_b` both map to `a_b`, `lib.rs:888-892`). Since
`rank_tasks` is an MCP tool reachable from a live frontend
(`crates/holon/src/api/block_domain.rs:505`), one garbage org drawer property
aborts the process instead of returning a tool error. Fail-loud is right;
fail-by-`panic!` in a library called from a server loop is not.

**Fix:** one `#[derive(thiserror::Error)]` enum per crate; convert the boundary
panics to `Err` and let the MCP layer surface them.

### C6 (reactive side) — ReactiveEngine is an admitted god-object with panicking trait defaults

`reactive.rs:1164`: `/// TODO: This looks like a god-class heavily violating SRP` —
directly above the many-field `ReactiveEngine`. `BuilderServices` carries
`unimplemented!()` defaults (`reactive.rs:71`, `:158`) whose doc comment concedes
the defaults exist only for two stub impls and that "headless" in the per-method
docs "is a misnomer" (`reactive.rs:43-53`). The `services_slot:
Arc<OnceLock<Arc<dyn BuilderServices>>>` self-reference hack (`reactive.rs:1188-1191`,
populated post-construction by each frontend) is a construction-order smell.
**Fix:** split `BuilderServices` into a required core + optional
`LazyWidgetServices` (kills both `unimplemented!()`s), and replace the OnceLock
self-slot with `Arc::new_cyclic` or a factory that returns the Arc directly.

## 3. Residue list

| # | Residue | Location | Fix |
|---|---------|----------|-----|
| R1 | Tombstone module: 4 lines of "removed, use ReactiveEngine" comments, still exported | `crates/holon-frontend/src/cdc.rs`, `pub mod cdc;` at `lib.rs:27`, echo comment at `lib.rs:101` | Delete file + decl |
| R2 | **Old and new models coexist**: waterui still calls deleted `holon_frontend::cdc::spawn_ui_listener` and old 2-arg `RenderContext::new`; crate is workspace-excluded (nominally for the naga/wgpu bug, root `Cargo.toml:33-39`) so it rots uncompiled | `frontends/waterui/src/lib.rs:73,138` | Migrate to ReactiveEngine or delete the frontend; do not let `exclude` hide a broken model |
| R3 | Orphan/tombstone state modules | `frontends/gpui/src/state.rs` (2 lines, not even declared in lib.rs); `frontends/ply/src/state.rs` (1 line) + `mod state;` at `frontends/ply/src/main.rs:4`; stale `frontends/ply/HANDOFF.md:35` ("AppState (identical to GPUI)") | Delete all three, fix HANDOFF |
| R4 | Unused dependency | `thiserror = "2"` in `crates/holon-engine/Cargo.toml`, zero uses | Use it (C5) or drop it |
| R5 | Acknowledged unfinished seams in the "complete cleanup" | `reactive.rs:429` (generation-field experiment TODO), `:1164` (god-class), `:2049` (`dispatch_intent` DRY), `:2055` ("preferences" special-case) | Triage: each is either a task or deleted |
| R6 | Dead knobs (see C4): empty `constraints`, unreferenced `discount`, inert energy/focus/mental-slots | `holon-petri/src/lib.rs:806,853-880`; `holon-engine/src/objective.rs:16-21` | Wire or cut |
| R7 | `f64::MAX` sentinel for "no deadline" flows into Rhai | `holon-petri/src/lib.rs:427-434` | Keep `Option`; emit `urgency_weight = 0` for `None` before eval |
| R8 | Float `Exact` precond compares via `String::parse` + 1e-9 epsilon | `holon-engine/src/guard.rs:143-148` | Parse `Exact` into `Value` at load (extend `PrecondSpec`) |

Checked and clean: no remaining compiled references to `AppState` /
`BlockWatchRegistry` / `CdcState` / `widget_states()` anywhere in the workspace
(tui, gpui, mcp all migrated; Blinc and ply also clean); the merge message
"Still broken (intentional): holon-tui, Ply/Blinc" was subsequently discharged —
except waterui (R2), which the message did not even list.

## 4. Coupling verdict

`holon-engine` is genuinely what the CrateMap claims (`lib.rs:6`: "Standalone
Petri-net engine"). Deps: clap, serde, serde_yaml, rhai, chrono, holon-expr — zero
Holon-domain tendrils; the generic `TokenState`/`Marking`/`NetDef` traits are a
clean seam and `holon-petri` is the sole domain adapter, consumed only by the fat
crate (`crates/holon/src/lib.rs:18` re-export → `block_domain.rs:505` →
MCP tool). `holon-expr` serde-as-source `CompiledExpr` with compile-on-
deserialize is a small, correct parse-boundary type. Layering: **pass.**

## 5. Overall verdict

**Not a "complete cleanup", but close on the surface it claims.** Inside the
workspace the reactive migration is real — old types are gone, not shadowed. The
seams left behind are: tombstone modules (R1, R3), one entire frontend still on the
dead model hidden by a workspace `exclude` (R2 — a direct violation of the "no old
code paths just in case" rule), and a god-object with panicking trait defaults that
the code itself annotates as debt (C6). The Petri engine is well-layered and the
abstraction fits its actual job (dependency-gated WSJF ranking) — the design debt
there is concentrated in holon-petri stringly token vocabulary (C3), the
Rhai-source-from-user-data construction (C2), the precond/postcond parse asymmetry
(C1), and a set of economics knobs that are modeled but never consulted (C4).
Priority order for follow-up: **C2 (correctness/injection) → C1 (fail-late +
per-fire parse) → R2 (coexisting models) → C5 (panic in MCP path) → C3/C4.**
