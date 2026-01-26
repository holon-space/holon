# Research: Rust Petri Net Libraries for Holon

## Requirements Summary

From docs/Vision/PetriNet.md, the Holon Petri Net engine needs:

1. **Colored/typed tokens** — tokens are Digital Twins with arbitrary attributes (energy, money, health, etc.)
2. **Guard conditions** on transitions — preconditions like `self.energy >= 0.3`
3. **Post-conditions** — effects applied after firing (attribute mutations, decay)
4. **Custom transition execution** — transitions that call MCP tools, spawn AI agents, interact with external systems
5. **Flexible storage** — state likely stored in Turso (libSQL), but should be pluggable
6. **Composite transitions** — sub-nets inside transitions (fractal structure)
7. **WSJF ranking** — custom scheduling/priority logic over enabled transitions
8. **Event sourcing** — append-only history, state derived from initial + replay
9. **Objective function** — scalar evaluation over all token attributes

## Evaluated Libraries

### 1. cpnsim (rwth-pads) ★1
**Repo:** https://github.com/rwth-pads/cpnsim
**Language:** Rust
**License:** MIT
**Last activity:** April 2025
**Maturity:** Early-stage, ~0 adoption

**What it does well:**
- Colored Petri Net simulation (the only Rust CPN implementation found)
- Uses **Rhai scripting** for guard evaluation and arc inscriptions — very relevant
- Parses `.ocpn` JSON format
- Has WASM compilation support
- Clean separation: `model.rs`, `simulator.rs`

**What it lacks:**
- Tokens are `HashMap<String, Vec<Dynamic>>` — Rhai's dynamic type, not extensible to custom Rust types
- No pluggable storage — everything in-memory
- No composite/hierarchical transitions
- No timed nets
- No post-conditions or effects system
- Tight coupling to `.ocpn` format
- Essentially zero community adoption

**Verdict:** Interesting as a reference for Rhai-based guard evaluation, but too tightly coupled to its own data model. Would need heavy forking.

### 2. pnets (LAAS/CNRS) ★9
**Repo:** https://github.com/Fomys/pnets
**Language:** Rust
**License:** LGPL-3.0
**Maturity:** Research tool, actively maintained for model checking competitions

**What it does well:**
- Supports both standard and **timed** Petri nets
- Clean typed IDs (`PlaceId`, `TransitionId`) with zero-cost newtype wrappers
- Adjacency list structure — efficient for graph traversal
- Net reduction algorithms (for analysis/verification)
- PNML and .net format parsing/export

**What it lacks:**
- **No colored tokens** — only integer markings (token counts)
- No guards, no custom transition logic
- Analysis-focused, not execution-focused
- No effects system
- LGPL license (less permissive)

**Verdict:** Good engineering but wrong abstraction level. This is for formal verification, not for running a production workflow engine.

### 3. netcrab ★~5
**Repo:** https://github.com/hlisdero/netcrab
**Language:** Rust
**License:** MIT / Apache 2.0
**Maturity:** Small project, educational

**What it does well:**
- Clean API with `PlaceRef` / `TransitionRef`
- `BTreeMap`-based for deterministic iteration
- Export to PNML, LoLA, DOT formats

**What it lacks:**
- Basic token counts only (no colored nets)
- No simulation engine — it's a net builder/exporter
- No guards, no custom logic, no storage

**Verdict:** Useful as a reference for Petri net graph data structures, not as a foundation.

### 4. petri.rs ★1
**Repo:** https://github.com/adri326/petri.rs
**Language:** Rust
**License:** MIT / GPL-3.0+

**What it does well:**
- State space generation and graph analysis
- Property verification (`always_reaches()`)
- Visualization via `petri_render`

**What it lacks:**
- Integer token counts only
- No colored nets, no guards, no custom logic
- Educational/analysis tool

**Verdict:** Not suitable.

### 5. petri-net-simulation
**Crate:** https://crates.io/crates/petri-net-simulation
**Language:** Rust

Built on top of `pns` crate. Designed for "non-linear plots in stories/games." Basic simulation with integer markings. Not suitable for our requirements.

### 6. nt-petri-net
**Repo:** https://github.com/MarshallRawson/nt-petri-net
**Language:** Rust

Non-deterministic transitioning Petri nets with colored tokens. Graph-based concurrent middleware. Has a debugging visualizer (PlotMux). Closest to our needs in spirit, but appears unmaintained and tightly coupled to its own runtime.

## Non-Rust Alternatives Worth Noting

### CPN Tools / Access/CPN
The gold standard for Colored Petri Nets. Java-based. Extremely mature but not embeddable in Rust.

### Renew (Reference Net Workshop)
Java-based. Supports "reference nets" (nets as tokens) — similar to our composite transitions concept. Academic, not embeddable.

### SNAKES (Python)
Python library for Petri nets with arbitrary Python objects as tokens and Python expressions as guards. Closest in philosophy to what we need, but wrong language.

## Key Technical Insight: Rhai for Guards

cpnsim's use of **Rhai** for guard evaluation is the most interesting finding. Rhai is:
- A Rust-native embedded scripting language (no FFI)
- Expression-focused (can disable loops/statements for pure guard evaluation)
- Supports registering Rust functions callable from scripts
- Compiles to AST for fast repeated evaluation
- Sandboxed and safe ("Don't Panic" guarantee)
- Serde-compatible for serialization
- No-std and WASM compatible

This is far better than hand-rolling a guard expression parser (as the current skill does). Guards like `self.energy >= 0.3 and self.focus >= 0.2` would be natural Rhai expressions with token attributes exposed as variables.

## Recommendation: Roll Our Own

**None of the existing libraries meet even half of our requirements.** The gap analysis:

| Requirement | cpnsim | pnets | netcrab | Others |
|------------|--------|-------|---------|--------|
| Colored/typed tokens | Partial (Dynamic) | ✗ | ✗ | ✗ |
| Guard conditions | ✓ (Rhai) | ✗ | ✗ | ✗ |
| Post-conditions/effects | ✗ | ✗ | ✗ | ✗ |
| Custom transition code | ✗ | ✗ | ✗ | ✗ |
| Pluggable storage | ✗ | ✗ | ✗ | ✗ |
| Composite transitions | ✗ | ✗ | ✗ | ✗ |
| Custom scheduling (WSJF) | ✗ | ✗ | ✗ | ✗ |
| Event sourcing | ✗ | ✗ | ✗ | ✗ |

### Proposed Architecture

A trait-based design with these core abstractions:

```rust
/// A token is a Digital Twin — any type that can report its attributes
trait Token: Send + Sync {
    fn id(&self) -> &TokenId;
    fn token_type(&self) -> &str;
    fn place(&self) -> &PlaceId;
    fn get_attr(&self, key: &str) -> Option<Value>;
    fn set_attr(&mut self, key: &str, value: Value);
}

/// Storage backend — could be in-memory, Turso, etc.
trait NetStore {
    fn tokens(&self) -> Vec<&dyn Token>;
    fn token(&self, id: &TokenId) -> Option<&dyn Token>;
    fn token_mut(&mut self, id: &TokenId) -> Option<&mut dyn Token>;
    fn append_event(&mut self, event: FiringEvent);
    fn events(&self) -> Vec<&FiringEvent>;
}

/// A transition can fire custom code
trait TransitionExecutor {
    /// Check preconditions beyond simple guards (e.g., external API availability)
    fn pre_check(&self, ctx: &FiringContext) -> Result<(), PreCheckError>;
    /// Execute the transition's side effects
    fn execute(&self, ctx: &mut FiringContext) -> Result<TransitionOutput, ExecutionError>;
    /// Post-conditions: validate output before committing
    fn post_check(&self, ctx: &FiringContext, output: &TransitionOutput) -> Result<(), PostCheckError>;
}

/// Scheduling strategy — WSJF is the default but pluggable
trait Scheduler {
    fn rank(&self, enabled: &[EnabledTransition], state: &NetState) -> Vec<RankedTransition>;
}
```

### What to Borrow

- **From cpnsim:** Rhai integration pattern for guard evaluation
- **From pnets:** Typed IDs (`PlaceId`, `TransitionId`) as newtype wrappers
- **From netcrab:** `BTreeMap` for deterministic iteration
- **From the YAML skill:** Event sourcing model, WSJF formula, mental slots effectiveness curve

### Implementation Phases

1. **Core engine** — Token trait, Place, Transition, Net graph, guard evaluation (Rhai), firing semantics
2. **Storage trait** — In-memory impl first, Turso impl second
3. **Scheduler** — WSJF as default `Scheduler` impl
4. **Transition executors** — Built-in (attribute mutation) + MCP tool executor + agent executor
5. **Composite transitions** — Sub-net as a transition (recursive structure)
6. **Event sourcing** — Append-only history, state derivation from initial + replay

### Crate Location

New crate: `crates/holon-petri/` — keeps it decoupled from the rest of holon initially, can be integrated later.

### Dependencies to Consider

- `rhai` — guard/expression evaluation
- `serde` + `serde_yaml`/`serde_json` — serialization
- `chrono` — time handling
- Existing holon crates for Turso storage integration
