---
title: holon-engine crate (Petri-net engine)
type: entity
tags: [crate, petri-net, wsjf, ranking, rhai]
created: 2026-04-13
updated: 2026-04-13
related_files:
  - crates/holon-engine/src/lib.rs
  - crates/holon-engine/src/engine.rs
  - crates/holon-engine/src/arc.rs
  - crates/holon-engine/src/objective.rs
  - crates/holon-engine/src/guard.rs
  - crates/holon-engine/src/value.rs
  - crates/holon-engine/src/main.rs
---

# holon-engine crate

A **standalone Petri-net engine** with no dependency on the `holon` crate. Provides the generic net execution model used by `holon::petri` for WSJF task ranking. Also exposes a CLI for YAML-defined nets.

## Core Traits

```rust
pub trait TokenState: Clone + Debug {
    fn id(&self) -> &str;
    fn token_type(&self) -> &str;
    fn get(&self, attr: &str) -> Option<&Value>;
    fn attrs(&self) -> &BTreeMap<String, Value>;
}

pub trait TransitionDef {
    fn id(&self) -> &str;
    fn inputs(&self) -> &[InputArc];
    fn outputs(&self) -> &[OutputArc];
    fn creates(&self) -> &[CreateArc];
}

pub trait NetDef {
    type T: TransitionDef;
    fn transitions(&self) -> &[Self::T];
    fn transition(&self, id: &str) -> Option<&Self::T>;
}

pub trait Marking {
    fn tokens(&self) -> impl Iterator<Item = &dyn TokenState>;
    fn get_token(&self, id: &str) -> Option<&dyn TokenState>;
    fn has_token(&self, id: &str) -> bool;
}
```

## Engine

`crates/holon-engine/src/engine.rs`

```rust
pub struct Engine {
    evaluator: RhaiEvaluator,
}
```

- `enabled(net, marking)` — finds all enabled transitions (those with valid token bindings for all input arcs)
- `fire(net, marking, binding, step)` — executes a transition: moves tokens per arcs, evaluates postconditions, records `Event`
- `rank(net, marking, objective)` — returns `Vec<RankedTransition>` sorted by `delta_per_minute` (WSJF = Δobj / duration); tiebreak: ascending alphabetical `transition_id`

### RankedTransition

```rust
pub struct RankedTransition {
    pub binding: Binding,
    pub delta_obj: f64,
    pub delta_per_minute: f64,
}
```

`Binding` maps arc bind-names to actual token IDs + captured placeholder values.

## Arcs

`crates/holon-engine/src/arc.rs`:
- `InputArc { place, bind, guard }` — consumes token from place, binds to name, evaluates Rhai guard
- `OutputArc { place, bind }` — produces token to place
- `CreateArc { token_type, attrs }` — creates a new token with Rhai-evaluated attributes

## Guards (Rhai)

`crates/holon-engine/src/guard.rs` — `RhaiEvaluator` executes Rhai expressions for arc guards and attribute expressions.

```rust
pub struct CompiledExpr(pub rhai::AST);
```

`CompiledExpr` is re-exported in `holon-api` for use in `FieldLifetime::Computed`.

Important Rhai gotchas (from production bugs):
- Rhai integer `3` does NOT match float `3.0` in switch statements
- Pass `priority` as float in context: use `3.0`, `2.0`, `1.0` literals in guards
- `format!("{}", 1.0_f64)` produces `"1"` — use `{:.6}` to force float notation in generated Rhai
- Always guard `is_def_var("completed_X") &&` before accessing completion token properties

## Objective Function

`crates/holon-engine/src/objective.rs` — computes the objective value for a marking. The WSJF objective is configurable via `prototype_for` blocks with Rhai expressions.

## Value Type

`crates/holon-engine/src/value.rs` — engine-internal `Value` enum (simpler than `holon-api::Value`, no FFI concerns). Used only within the engine.

## YAML CLI

`crates/holon-engine/src/main.rs` — CLI for running YAML-defined Petri nets. Supports:
- `--net net.yaml` — load net definition
- `--what-if` — simulate transitions without committing
- `--rank` — show WSJF ranking

YAML format in `crates/holon-engine/src/yaml/`.

## Canary Blocks (PBT)

PBT injects two canary transitions per run:
- `canary_aaa` — no priority, position 0 (should rank LAST if weights work)
- `canary_zzz` — priority 3, position 1 (should rank FIRST)

Naming is intentional: alphabetical tiebreak (`aaa` < `zzz`) works AGAINST correct ordering when weights are equal, so canary ordering validates weights are actually computed.

## Related Pages

- [[concepts/petri-net-wsjf]] — WSJF design and prototype blocks
- [[entities/holon-crate]] — `holon::petri` materializes blocks into this engine's types
