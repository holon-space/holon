//! @c4 component
//! @c4 layer Core
//! Pattern: Shared Kernel
//!
//! The dual-evaluated guard `Pattern` AST (ADR 0024 / ADR 0031) and the
//! dynamic `Value` it embeds. A leaf crate: `holon-macros` may depend on it to
//! parse guard strings at macro-expansion time, which `holon-api` (a dependent
//! of `holon-macros`) can never allow.
//!
//! `holon-api` re-exports both, so `holon_api::pattern::*` and
//! `holon_api::Value` stay the canonical consumer paths.

pub mod pattern;
mod value;

pub use value::Value;
