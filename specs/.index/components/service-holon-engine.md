---
name: holon-engine
description: Petri net state machine engine with Rhai/YAML expression evaluation
type: reference
source_type: component
source_id: crates/holon-engine/src/
category: service
fetch_timestamp: 2026-04-23
---

## holon-engine (crates/holon-engine)

**Purpose**: Petri net simulator and expression engine. Models workflows as place/transition nets with Rhai-based guard expressions.

### Key Modules & Types

| Module | Key Types |
|--------|-----------|
| `engine` | `Engine` — drives net execution, fires transitions |
| `arc` | `InputArc`, `OutputArc`, `CreateArc` — net topology |
| `guard` | `CompiledExpr` — Rhai expression compiled for guard evaluation |
| `yaml` | `YamlNet`, `YamlNetFile`, `YamlMarking`, `YamlTransition`, `YamlToken` |
| `objective` | `ObjectiveDef`, `ObjectiveResult` — optimization objectives |
| `value` | Value type system for token data |
| `display` | Debug display formatting |

### Key Traits

| Trait | Role |
|-------|------|
| `TokenState` | State carried by Petri net tokens |
| `TransitionDef` | Defines a transition's guards and effects |
| `NetDef` | Full net definition |
| `Marking` | Current token distribution across places |

### Architecture Notes

- YAML net files define workflow graphs with Rhai guard expressions
- `Engine::fire()` evaluates all enabled transitions and applies token moves
- Rhai expressions are compiled once (`CompiledExpr`) and re-evaluated per marking
- Integrated into `holon` crate via the `petri` module

### Vision

See `VISION_PETRI_NET.md` — Petri nets model personal workflow automation (task routing, focus management, etc.)

### Keywords
petri-net, state-machine, Rhai, workflow, engine, tokens, transitions
