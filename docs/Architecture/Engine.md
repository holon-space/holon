# Petri-Net Engine & Platform

*Part of [Architecture](../Architecture.md)*

## Standalone Petri-Net Engine (`holon-engine`)

The `holon-engine` crate is a standalone CLI binary for Petri-net simulation and WSJF task ranking. It has **no dependency** on the `holon` crate — it operates purely on YAML files.

**Location**: `crates/holon-engine/`

### Core Traits

```rust
pub trait TokenState    { fn id(&self) -> &str; fn token_type(&self) -> &str; fn get(&self, attr: &str) -> Option<&Value>; fn attrs(&self) -> &BTreeMap<String, Value>; }
pub trait TransitionDef { fn id(&self) -> &str; fn inputs(&self) -> &[InputArc]; fn outputs(&self) -> &[OutputArc]; fn creates(&self) -> &[CreateArc]; fn duration_minutes(&self) -> f64; }
pub trait NetDef        { fn transitions(&self) -> &[impl TransitionDef]; }
pub trait Marking       { fn tokens(&self) -> Vec<&dyn TokenState>; fn add_token(...); fn remove_token(...); }
```

### Key Components

| Component | File | Purpose |
|-----------|------|---------|
| `Engine` | `engine.rs` | Core simulation: `enabled()` finds fireable bindings, `fire()` executes a transition, `rank()` produces WSJF-ordered `RankedTransition` list |
| `RhaiEvaluator` | `guard.rs` | Rhai-based guard/precondition evaluation, postcondition attribute updates, compiled expression caching |
| `ObjectiveResult` | `objective.rs` | Evaluates objective function over current marking state |
| `YamlNet` | `yaml/net.rs` | YAML-defined net with transitions, arcs, and objective function |
| `YamlMarking` | `yaml/state.rs` | YAML-serialized token state (load/save) |
| `History` | `yaml/history.rs` | Append-only event log with replay support |

### Relationship to `holon/src/petri.rs`

`petri.rs` in the main `holon` crate materializes blocks into Petri-net structures for WSJF ranking. It depends on `holon-engine` for the core simulation logic. The standalone `holon-engine` binary allows running Petri-net simulations independently of the full Holon application.

## Ordering with Fractional Indexing

Block ordering uses fractional indexing:
- Sort keys are base-26-like strings
- Supports arbitrary insertion without rewriting all keys
- Automatic rebalancing when keys get too long

## Platform Support

### WASM Compatibility

- `MaybeSendSync` trait alias relaxes Send+Sync on WASM
- `#[async_trait(?Send)]` for non-Send futures
- Conditional compilation for platform-specific features

### Supported Frontends

| Frontend | Status | Notes |
|----------|--------|-------|
| GPUI | Primary | Native Rust GUI (runs on Android via Dioxus), embeds MCP server |
| Flutter | Active | FFI bridge via flutter_rust_bridge |
| Blinc | Active | Native Rust GUI via blinc-app |
| MCP | Active | Model Context Protocol server (stdio + HTTP modes) |
| Dioxus | Experimental | Dioxus-based frontend |
| Ply | Experimental | Ply-based frontend |
| TUI | Experimental | Terminal UI frontend |
| WaterUI | Experimental | WaterUI-based frontend |

## Consistency Model

### Local Consistency
- Database transactions ensure atomic updates
- CDC delivers changes in commit order
- UI reflects committed state

### External Consistency
- Eventually consistent (5-30 second typical delay)
- Last-write-wins for concurrent edits
- Sync tokens prevent duplicate processing

