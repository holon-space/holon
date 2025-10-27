//! Value functions — DSL-level functions that return `InterpValue`.
//!
//! A value function is `RenderExpr::FunctionCall { name, args }` whose
//! return type is a plain `Value` (scalar) or `Value::Rows` (reactive
//! row-set provider) — as opposed to widget builders, which return a
//! widget node `W`.
//!
//! Dispatch: registered in `RenderInterpreter::register_value_fn`.
//! Invoked during arg evaluation via `ValueFnBinding`.
//! Names must not collide with widget builders (enforced at register
//! time).
//!
//! Currently registered value fns: `ops_of`, `focus_chain`,
//! `chain_ops`. Each lives in its own submodule and exposes a
//! `register_*` entrypoint called from `build_shadow_interpreter`.

pub mod chain_ops;
pub mod focus_chain;
pub mod ops_of;
pub mod state_accent;
pub mod synthetic;

pub use chain_ops::register_chain_ops;
pub use focus_chain::register_focus_chain;
pub use ops_of::register_ops_of;
pub use state_accent::register_state_accent;
