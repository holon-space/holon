# Adversarial review: is the 22-crate decomposition optimal?

*Fable, 2026-07-06. Role: contrarian principal engineer. Method: measured the real
Cargo dep graph (`cargo metadata`, normal vs dev vs optional edges), LoC per crate,
`ast-outline cycles`, archlint rules, and the @c4 CrateMap claims — then argued
against the design, steel-manned it, and rendered a verdict.*

---

## 1. Ground truth (measured, not from the docs)

**Prod library crates (17):** holon (18.4k LoC), holon-api (14.8k), holon-app (3.0k),
holon-core (6.4k), holon-engine (1.7k), holon-expr (0.07k), holon-filesystem (3.8k),
holon-frontend (27.8k / 91 files), holon-loro (17.3k), holon-macros (4.1k),
holon-markdown (2.7k), holon-mcp-client (6.7k), holon-org-format (3.8k),
holon-orgmode (2.4k), holon-petri (1.3k / 1 file), holon-profiles (1.9k / 2 files),
holon-turso (7.1k). Plus frontends/mcp (4.8k) which is de facto a library (below).

**Testing crates (6):** integration-tests (52.3k / 221 files), layout-testing,
pbt-core, block-roundtrip-testing (750 LoC / 1 file), architecture-tests (7 LoC),
macros-test.

**Internal normal edges among prod crates: 58** (excluding workspace-hack, dev-deps,
test crates). Counting frontends/mcp-as-library's 9 edges: **67**.

**Feature-gating is real:** `holon-frontend -> holon-pbt-core` is optional behind
`pbt`; `holon-gpui -> holon-integration-tests` is optional behind `pbt`;
integration-tests' 15 workspace deps are all optional behind `test-infra`/`pbt`.
The old "proptest in prod builds" finding has been fixed at these seams.

**File-level cycles (ast-outline):** a 5-file cycle inside holon-frontend
(reactive.rs <-> reactive_view_model.rs <-> render_context.rs <-> render_interpreter.rs <->
view_model.rs), plus 2-file cycles in holon (backend_engine <-> di/test_helpers),
holon-integration-tests, and holon-loro. No crate-level cycles (Cargo forbids them).

**Enforcement reality:** archlint = micro-lints (`.ok()`, `filter_map_ok`,
`block_on`-in-async, jsonb, underscore params). The CrateMap "Layer" column is a
doc-baseline check (`arch-validate`), not a dependency rule. The only layering
actually *enforced* is Cargo's acyclicity.

---

## 2. The adversarial case

### C1 — `holon` is a shadow composition root; the CrateMap lies about holon-app

CrateMap: *"holon-app ... owns **every** wiring that names concrete backends."*
Measured: `holon` normal-deps on **holon-loro, holon-turso, holon-petri,
holon-profiles, holon-engine**, and names `holon_turso`/`holon_loro` in **12 files**,
including `crates/holon/src/di/registration.rs`, `di/schema_providers.rs`,
`sync/loro_module.rs`, `storage/mod.rs`, `api/memory_backend.rs`. There is a whole
`holon/src/di/` directory *outside* the composition root. Whatever holon-app's
discipline is, it is not compile-time true: a second, bigger wiring layer lives one
crate below it. Either holon-app is redundant or holon is mislabeled "Facade".

### C2 — Adapter-to-adapter edge: holon-mcp-client -> holon-turso (and -> rhai)

`crates/holon-mcp-client/src/mcp_integration.rs:12` imports
`holon_turso::turso::DbHandle` and calls
`holon_turso::matview_manager::reconcile_named_view` (line 580);
`mcp_sync_engine.rs:15` likewise. The "reusable MCP client" adapter is welded to one
specific storage adapter. It also deps holon-profiles -> holon-engine -> rhai, so the
MCP client transitively compiles a scripting engine. This is the one genuine
hexagon violation in the graph — and nothing (archlint, layer docs) flagged it,
which proves C7 below.

### C3 — Two-headed fat kernel: holon-api + holon-core

holon-api (14.8k LoC) + holon-core (6.4k) are both "shared kernel"; core depends on
api, and essentially every adapter deps *both* (turso, loro, org-format, markdown,
petri, profiles, filesystem, frontend...). The types-vs-traits boundary between them
enforces nothing — no crate consumes core without api. Meanwhile holon-api drags
`rhai` (via holon-expr, `sync` feature) to the bottom of the entire graph: every
clean build of anything compiles a scripting engine. And the names: **`holon`,
`holon-core`, `holon-api` all mean "the middle"** — worse, `crates/holon/src/` contains
directories literally named `api/` and `core/` while sibling crates `holon-api` and
`holon-core` exist. For an AI (or human) asking "where does `Operation` live?",
this is a three-way coin flip. Navigability failure by naming alone.

### C4 — holon-markdown is a dead crate

2.7k LoC, **zero dependents** (no Cargo.toml in the workspace references it;
Model.md itself admits: *"crates/holon-markdown exists but is unwired — no crate
depends on it"*). The repo's own doctrine (CLAUDE.md: delete old code paths, the
tree is the strongest signal to the next agent) says this shouldn't exist. Every
agent that greps "FileFormatAdapter" finds a second impl that is a mirage.

### C5 — The Petri feature is smeared across four crates

holon-expr (66 LoC!), holon-engine (1.7k), holon-petri (1.3k, a *single file*),
holon-profiles (1.9k, 2 files) — ~5k LoC and 4 Cargo.tomls for one subsystem.
And the stated justification for holon-expr ("shared vocabulary between holon-api
and the engine") is half-hollow: holon-profiles *bypasses* CompiledExpr, storing raw
source strings and re-compiling on demand (its own header comment says Rhai ASTs
are !Send+!Sync — stale, given expr uses rhai/`sync`). The vocabulary isn't even
shared by the crate that most needed it.

### C6 — holon-frontend is the god crate, and frontends/mcp is a mislabeled library

The largest prod crate (27.8k / 91 files) has the *least* internal structure: the
only 5-file import cycle in the workspace sits at its heart (reactive <-> view_model <->
render_interpreter). It also normal-deps holon-filesystem — why does a ViewModel
need the filesystem port? Meanwhile `frontends/mcp` is consumed as a **normal dep**
by holon-gpui, holon-tui, and holon-integration-tests. A "frontend"
with four dependents is a library; the directory taxonomy misleads.

### C7 — Layering is documentation, not enforcement

archlint enforces expression-level smells; @c4 layers are prose validated against a
doc baseline. Nothing prevents the next adapter-to-adapter edge. C2 already happened
silently. A layout this proud of its layer table should have at least a
cargo-deny-style ban list or an archlint aggregate rule over Cargo.tomls.

---

## 3. The alternative: 13-crate "hourglass"

Optimized for: compile-enforced layering, rebuild blast radius, AI-navigability
(names that say what they are), testability (keep the pbt gate seams).

| New crate | Formed from | Normal deps |
| --- | --- | --- |
| `holon-macros` | unchanged (proc-macro must stand alone) | — |
| `holon-expr` | unchanged (see steel-man S3) | — |
| `holon-kernel` | holon-api + holon-core merged | macros, expr |
| `holon-fs` | holon-filesystem | kernel, macros |
| `holon-turso` | unchanged | kernel |
| `holon-loro` | unchanged | kernel, fs, macros |
| `holon-org` | org-format + orgmode merged; disk I/O behind an `io` feature (pure half stays wasm-clean for the dioxus-web worker) | kernel, fs*, macros |
| `holon-engine` | engine + petri + profiles (the whole Petri/WSJF/profile subsystem, ~5k LoC, one home) | kernel, expr |
| `holon-mcp-client` | unchanged code, but the Turso weld (C2) replaced by a `ViewReconciler`/`DbHandle` port in kernel, impl passed by holon-app | kernel, macros |
| `holon-sync` | **rename of `holon`**, minus `src/di/` (consolidator, sync pipeline, BackendEngine, storage). Legitimately names Loro/Turso — Model.md layers 2–3 list them as members | kernel, fs, loro, turso, engine, macros |
| `holon-viewmodel` | rename of holon-frontend; phase 2 (after breaking the 5-file cycle) splits out `holon-reactive` | kernel, fs, macros (+pbt-core opt.) |
| `holon-mcp-server` | frontends/mcp moved to crates/; frontends/mcp becomes a thin bin | kernel, app, viewmodel |
| `holon-app` | composition root, **now absorbing holon/src/di/** — the CrateMap claim becomes true | everything above |

Deleted: `holon-markdown` (VCS remembers; reborn as `holon-md` on the org pattern
when actually wired). Testing crates: **unchanged** (see S1).

**Edge count: 67 -> ~37** (−45%). Crate count 18 -> 13. Every remaining name answers
"what is this?" without opening lib.rs: kernel / fs / turso / loro / org / engine /
mcp-client / sync / viewmodel / mcp-server / app. No more holon-vs-holon-core-vs-
holon-api coin flip; no more `holon/src/api` shadowing `holon-api`.

Blast-radius note: merging api+core is ~free — nearly every dependent of core
already deps api, so the rebuild frontier is unchanged; what improves is the
navigation and the honesty. The expensive hub (kernel at the bottom) exists in both
designs; no reshuffle removes it without splitting *types by domain*, which the
Operation/entity macro system makes impractical today.

---

## 4. Steel-man: what the current design gets right

**S1 — The pbt feature-gate discipline is genuinely good.** `holon-frontend/pbt`,
`holon-gpui/pbt`, and integration-tests' all-optional dep set are exactly how you
let ONE composed PBT reach into prod crates without shipping proptest. A naive
"move all test code out" reshuffle would break the keystone-PBT architecture the
project has spent weeks converging on. My proposal deliberately doesn't touch it.

**S2 — org-format vs orgmode is the *principled* split**, not the arbitrary one:
pure parse/render/diff (wasm-compilable, used by block-roundtrip generators) vs
disk I/O. The dioxus-web worker is a live consumer of the pure half. My merge into
`holon-org` must keep that seam as a feature gate or it regresses a real target.
Markdown *not* having the split isn't inconsistency — it's an unwired stub.

**S3 — holon-expr's 66 LoC is the textbook minimal cycle-cut**, not fragmentation.
Entity definitions in the kernel need `CompiledExpr` (compile-at-deserialization =
parse-don't-validate, a core doctrine here); the engine needs it too; kernel->engine
would be absurd, engine->kernel exists conceptually. The alternatives are worse:
raw strings in the kernel (weakens the doctrine) or rhai types inlined into the
kernel (couples harder). The cost is one Cargo.toml. Keep it. (The rhai-in-
every-build cost is real but a one-time clean-build tax; expr churns ~never.)

**S4 — The consolidator naming Loro/Turso is by design, not a violation.**
Model.md layers 2–3 explicitly list Loro and Turso as *members* of the consolidator
and projection layers. The hexagon claim only ever applied to file formats,
frontends, and external providers. So C1's honest form is narrower: not "the
architecture is fake" but "the DI modules live in the wrong crate" — which is a
one-directory move, not a redesign.

**S5 — Granularity buys parallel compile, per-crate nextest, per-crate mutants
gating** (the mutants campaign is run crate-by-crate). Fewer, fatter crates
serialize compilation and coarsen the mutation/coverage gates.

**S6 — The @c4/arch-validate loop is cheap and has caught drift.** Docs-as-baseline
is weaker than enforcement but far better than nothing, and it produced the very
CrateMap this review could hold the code accountable to.

---

## 5. Verdict

**The current decomposition is ~80% sound; a wholesale redesign is not worth it —
targeted surgery is.** The load-bearing seams (adapter crates, pbt feature gates,
pure-vs-IO org split, the expr cycle-cut) are correct and hard-won. The genuine
defects are five, and four are cheap:

| # | Fix | Cost | Benefit |
| --- | ----- | ------ | --------- |
| 1 | Move `crates/holon/src/di/` -> holon-app (makes the composition-root claim true) | ~1/2 day | Kills C1; CrateMap stops lying |
| 2 | Cut `holon-mcp-client -> holon-turso` via a kernel port | ~1 day | Kills the only real hexagon violation (C2) |
| 3 | Delete `holon-markdown` | ~10 min | Kills C4; tree stops mis-signaling |
| 4 | Move frontends/mcp -> `crates/holon-mcp-server` | ~1/2 day | Kills C6b; taxonomy honesty |
| 5 | Add an archlint aggregate rule over Cargo.tomls banning adapter-to-adapter edges | ~1/2 day | Kills C7; makes the layer table enforceable |

**Deferred (churn > benefit right now):** merging api+core into `holon-kernel` and
renaming `holon` -> `holon-sync` is the biggest AI-navigability win in this document,
but it is a 100+-file import-path churn across every crate and every open worktree,
mid-PBT-endgame and mid-display-placement work. Do it as a dedicated mechanical
session (opus-worker grade) after G1, not now. Folding petri+profiles into engine:
low value, do opportunistically. Splitting holon-frontend: blocked on breaking its
5-file cycle first — the cycle, not the crate boundary, is the actual debt there.

Locally optimal? No — items 1–5 are strict improvements. Globally redesign-worthy?
Also no. The graph's shape is right; its labels and three edges are wrong.
