//! @c4 component
//! @c4 layer Core
//! @c4 uses holon-lens "quantity value type and text formats" "Rust"
//! Pattern: Shared Kernel
//!
//! The dual-evaluated guard `Pattern` AST (ADR 0024 / ADR 0031) and the
//! dynamic `Value` it embeds. Sits below `holon-api`: `holon-macros` may depend
//! on it to parse guard strings at macro-expansion time, which `holon-api` (a
//! dependent of `holon-macros`) can never allow.
//!
//! `holon-api` re-exports both, so `holon_api::pattern::*` and
//! `holon_api::Value` stay the canonical consumer paths.

pub mod arcs;
pub mod kind_envelope;
pub mod marking;
pub mod pattern;
mod property_kinds;
pub mod schema;
mod value;

pub use property_kinds::AmbiguousKind;
pub use property_kinds::PropertyKinds;
pub use property_kinds::PropertyKindsError;
pub use value::NotJson;
pub use value::REMOVED_MARKER_KEY;
pub use value::RemovedTag;
pub use value::Value;
