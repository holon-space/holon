# Petri-Net Engine & Platform

*Part of [Architecture](../Architecture.md)*

## Standalone Petri-Net Engine (`holon-engine`)

The `holon-engine` crate is a standalone CLI binary for Petri-net simulation and WSJF task ranking. It has **no dependency** on the `holon` crate — it operates purely on YAML files.

**Location**: `crates/holon-engine/`

### Core Traits

```rust
pub trait TokenState    { fn id(&self) -> &str; fn token_type(&self) -> &str; fn get(&self, attr: &str) -> Option<&Value>; fn attrs(&self) -> &BTreeMap<String, Value>; }
pub trait TransitionDef { fn id(&self) -> &str; fn inputs(&self) -> &[InputArc]; fn outputs(&self) -> &[OutputArc]; fn creates(&self) -> &[CreateArc]; fn duration_minutes(&self) -> f64; }
pub trait NetDef        { type Transition: TransitionDef; fn transitions(&self) -> Box<dyn Iterator<Item = &Self::Transition> + '_>; fn transition(&self, id: &str) -> Option<&Self::Transition>; fn objective_expr(&self) -> &CompiledExpr; fn constraints(&self) -> &[CompiledExpr]; fn discount_rate(&self) -> f64; }
pub trait Marking: Clone { type Token: TokenState; fn clock(&self) -> DateTime<Utc>; fn set_clock(&mut self, t: DateTime<Utc>); fn tokens(&self) -> Box<dyn Iterator<Item = &Self::Token> + '_>; fn tokens_of_type(&self, token_type: &str) -> Vec<&Self::Token>; fn token(&self, id: &str) -> Option<&Self::Token>; fn set_attr(&mut self, token_id: &str, attr: &str, value: Value); fn create_token(&mut self, id: String, token_type: String, attrs: BTreeMap<String, Value>); fn remove_token(&mut self, id: &str); }
```

(Authoritative source: `crates/holon-engine/src/lib.rs` — regenerate with `ast-outline outline crates/holon-engine/src/lib.rs` if this drifts.)

### Key Components

| Component | File | Purpose |
|-----------|------|---------|
| `Engine` | `engine.rs` | Core simulation: `enabled()` finds fireable bindings, `fire()` executes a transition, `rank()` produces WSJF-ordered `RankedTransition` list |
| `RhaiEvaluator` | `guard.rs` | Rhai-based guard/precondition evaluation, postcondition attribute updates, compiled expression caching |
| `ObjectiveResult` | `objective.rs` | Evaluates objective function over current marking state |
| `YamlNet` | `yaml/net.rs` | YAML-defined net with transitions, arcs, and objective function |
| `YamlMarking` | `yaml/state.rs` | YAML-serialized token state (load/save) |
| `History` | `yaml/history.rs` | Append-only event log with replay support |

### Relationship to `crates/holon-petri`

The `holon-petri` crate materializes task blocks into Petri-net structures for WSJF ranking (tokens = entities, transitions = tasks; see its `lib.rs` module doc). It depends on `holon-api`, `holon-core`, and `holon-engine`, and is re-exported by the fat crate as `holon::petri` (`crates/holon/src/lib.rs`). The standalone `holon-engine` binary allows running Petri-net simulations independently of the full Holon application.

## Ordering with Fractional Indexing

Block ordering uses fractional indexing:
- Sort keys are hex-encoded fractional indices minted via the `loro_fractional_index` crate (`gen_key_between` in `crates/holon-core/src/fractional_index.rs`; `DEFAULT_SORT_KEY = "A0"`). The crate dependency is deliberately kept — see the module-doc rationale in that file; do not hand-roll key generation.
- Supports arbitrary insertion without rewriting all keys
- The only production rebalancing is the tied-key rebalance in `crates/holon/src/core/sql_block_operations.rs`, which triggers on duplicate sibling keys — there is no length-based rebalancing (`MAX_SORT_KEY_LENGTH` is asserted only in `fractional_index.rs` tests)

## Platform Support

### WASM Compatibility

- `MaybeSendSync` is `Send + Sync` on **all** targets (`crates/holon-core/src/traits.rs`) — the historical wasm relaxation was removed because the wasm32 browser demo uses Arc/Mutex-backed types; do not reintroduce the cfg split
- `#[async_trait(?Send)]` survives in two forms: (a) unconditionally in the capmap macro output (`crates/holon-macros/src/capmap.rs:105`) and the cap-trait impls it patterns in test crates (`holon-integration-tests` / `holon-pbt-core` — ~109 occurrences across ~23 files); (b) wasm32-gated via `#[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait(?Send))]` in `crates/holon-core` (`entity_cache.rs:21,49`, `operation_wrapper.rs:128,152`)
- Conditional compilation for platform-specific features

### Supported Frontends

| Frontend | Status | Notes |
|----------|--------|-------|
| GPUI | Primary | Desktop. Mobile via `gpui-mobile` optional dep (`frontends/gpui/Cargo.toml`); screen-layout optimization ongoing. Embeds MCP server. |
| TUI | Active | Keyboard-driven terminal UI. |
| MCP | Active | Model Context Protocol server (stdio + HTTP modes). |
| Dioxus / dioxus-web | Prototype | Core works; not actively tested. Both are currently in the workspace `exclude` list (root `Cargo.toml`): `dioxus` temporarily (cocoa version conflict with gpui — see the TEMP comment), `dioxus-web` permanently (wasm32-only, built via `trunk`). |
| Flutter | Deprecated | Directory removed; integrating a second language/toolstack was too painful. |
| Ply / WaterUI | Excluded from workspace | Upstream compatibility issues. |
| Blinc | — | `blinc` feature flag in `crates/holon-frontend/Cargo.toml`, not a frontend directory; excluded from workspace due to upstream compatibility issues. |

## Consistency Model

### Local Consistency
- Database transactions ensure atomic updates
- CDC delivers changes in commit order
- UI reflects committed state

### External Consistency
- Eventually consistent (5-30 second typical delay)
- Last-write-wins for concurrent edits
- Sync tokens prevent duplicate processing

