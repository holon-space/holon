---
title: Petri Net & WSJF Task Ranking
type: concept
tags: [petri-net, wsjf, ranking, rhai, prototype-blocks]
created: 2026-04-13
updated: 2026-04-13
related_files:
  - crates/holon/src/petri.rs
  - crates/holon-engine/src/engine.rs
  - crates/holon-engine/src/arc.rs
  - crates/holon-engine/src/objective.rs
  - VISION_PETRI_NET.md
---

# Petri Net & WSJF Task Ranking

Holon materializes task blocks into a Petri Net to compute WSJF (Weighted Shortest Job First) rankings. This is "structural intelligence" — the scoring logic lives in data structures, not AI models.

## WSJF Formula

```
WSJF = ΔObjective / Duration
```

Higher `delta_per_minute` = do this task first.

Tiebreak: ascending alphabetical `transition_id` (deterministic across runs).

## Petri Net Model

In the Holon Petri Net:
- **Tokens** = entities (the user, referenced people, documents, monetary items, knowledge)
- **Transitions** = tasks (with dependency arcs)
- **Markings** = which tokens are where (what has been completed, delegated)
- **Objective function** = the WSJF scoring expression (Rhai)

### Token Types

`TaskToken` with `token_type` string:
- `Person` — human actor
- `Document` — knowledge artifact
- `Monetary` — financial item
- `Resource` — physical/digital resource
- `Organization` — company/team

### Content Prefix Parsing

Parsed left-to-right, each strips its marker:
1. `>` — sequential dependency on previous sibling (this task can't start until sibling is done)
2. `@[[Person]]:` — delegation to another person
3. `?` — question producing a knowledge token

## Prototype Blocks

Prototype blocks replace the old `MaterializeConfig`. A block with `prototype_for` property defines defaults and computed fields.

```org
* DEFAULT_TASK_PROTOTYPE
  :PROPERTIES:
  :prototype_for: task
  :priority_weight: =switch priority { 3.0 => 10.0, 2.0 => 5.0, 1.0 => 2.0, _ => 1.0 }
  :urgency_weight: =1.0
  :position_weight: =1.0 / (position + 1.0)
  :task_weight: =priority_weight * urgency_weight * position_weight
  :END:
```

`=`-prefixed values are Rhai expressions, evaluated at materialization time. Plain values are literal defaults.

`DEFAULT_TASK_PROTOTYPE` is a Rust const in `crates/holon/src/petri.rs` defining the baseline scoring.

## resolve_prototype

```rust
pub fn resolve_prototype(
    proto_props: BTreeMap<String, String>,
    instance_props: BTreeMap<String, String>,
    context_props: BTreeMap<String, rhai::Dynamic>,
) -> BTreeMap<String, Value>
```

1. Merges `instance_props` over `proto_props` (instance overrides prototype)
2. Topological-sorts computed (`=`-prefixed) fields by dependency
3. Evaluates each Rhai expression in order, with previously computed values in scope
4. Returns final resolved property map

## SelfDescriptor

Reads the user's own token from a block with `is_self: true`. Falls back to `SelfDescriptor::defaults()` if not found.

`rank_tasks()` scans DB for:
- `prototype_for IS NOT NULL` blocks → prototype definitions
- `is_self = true` block → self descriptor

## Materialization Flow

`crates/holon/src/petri.rs::materialize_at(blocks, self_desc, prototype_props, now)`:

1. Scan all blocks for task_state (filter completed tasks)
2. For each task block, call `resolve_prototype()` to get final scores
3. Create `TaskToken` for self + any referenced entities
4. Create `TaskTransition` for each task with `InputArc` (requires self token) and `CreateArc` (completion token on done)
5. Build `NetDef` + initial `Marking`
6. Call `Engine::rank(net, marking, objective)` → `Vec<RankedTransition>`

## Canary Blocks (PBT Validation)

PBTs inject two canary transitions:
- `canary_aaa` — no priority, position 0 → should rank LAST
- `canary_zzz` — priority 3, position 1 → should rank FIRST

Naming is intentional: alphabetical tiebreak works **against** correct ordering when weights are equal. If `canary_zzz` appears before `canary_aaa` in the ranked output, weights are actually being computed.

## Rhai Type System Gotchas

These bugs have been fixed but are worth documenting:

1. **Integer vs float switch**: `switch 3.0 { 3 => ... }` does NOT match — `3` is int, `3.0` is float. Always use `3.0`, `2.0`, `1.0` float literals.
2. **Float formatting**: `format!("{}", 1.0_f64)` produces `"1"` not `"1.0"`. Use `{:.6}` to force float notation in generated Rhai.
3. **Undefined variable guard**: Always use `is_def_var("completed_X") &&` before accessing completion token properties.

## Related Pages

- [[entities/holon-engine]] — standalone Petri net engine
- [[entities/holon-crate]] — `petri.rs` materialization
- [[entities/holon-integration-tests]] — canary block PBT invariants
