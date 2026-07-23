//! `holon-toon` — an experiment crate.
//!
//! A dense, round-trippable **TOON** projection of the Holon block forest,
//! built as a structural sibling of `crates/holon-org-format` so it can be
//! promoted directly if the experiment wins. The library parses and renders
//! the projection an agent reads and patches; `MAPPING.md` records the
//! org<->TOON construct mapping and `examples/measure.rs` measures token cost
//! against org on real vault files.
//!
//! Layers (bottom to top):
//! - [`toon`] — generic TOON scalar quoting/escaping, row split, props codec.
//! - [`schema`] — the fixed 6-column tabular schema.
//! - [`models`] — the block-forest domain types (parse-don't-validate
//!   newtypes).
//! - [`renderer`] / [`parser`] — `Forest` <-> TOON text.

pub mod error;
pub mod models;
pub mod org_reader;
pub mod parser;
pub mod renderer;
pub mod schema;
pub mod toon;

pub use error::Result;
pub use error::ToonError;
pub use models::BlockId;
pub use models::BlockNode;
pub use models::ContentType;
pub use models::Forest;
pub use models::Priority;
pub use models::TaskState;
pub use models::ToonBlock;
pub use parser::parse;
pub use renderer::render;
