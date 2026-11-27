//! @c4 component
//! @c4 layer Engine
//! Pattern: Interpreter
//!
//! Workflow nets over a token value `V`: the executor of runtime nets with
//! effects, retry, timers, failure ports, undo and asks, the net file format,
//! its loader and probes. The host passes the functions that meet `V` in a
//! [`exec::Host`], so `V` needs no trait beyond `Clone + PartialEq + Debug`.
//!
//! The crate does not check workflow-net soundness.

pub mod exec;
pub mod file;
pub mod ids;
pub mod load;
pub mod probe;

pub use ids::AnswerId;
pub use ids::AskHash;
pub use ids::OccurrenceId;
