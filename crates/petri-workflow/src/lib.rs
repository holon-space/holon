//! @c4 component
//! @c4 layer Engine
//! Pattern: Interpreter
//! @c4 uses holon-transition-contract "occurrence, answer and ask-hash ids"
//! "Rust"
//!
//! Workflow nets over a token value `V`: the executor of runtime nets with
//! effects, retry, timers, failure ports, undo and asks, the net file format,
//! its loader and probes. The host passes the functions that meet `V` in a
//! [`exec::Host`], so `V` needs no trait beyond `Clone + PartialEq + Debug`.
//!
//! The crate does not check workflow-net soundness.

pub mod exec;
pub mod file;
pub mod load;
pub mod probe;
