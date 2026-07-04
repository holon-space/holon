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
//!   This is the single escaping implementation the whole crate funnels
//!   through.
//! - [`table`] — the **generic tabular codec**: `Vec<Row>` (`Row =
//!   BTreeMap<String, ToonValue>`) <-> TOON text, with a deterministic sorted
//!   column union and an explicit absent-vs-empty distinction. This is TOON's
//!   real sweet spot (uniform value-heavy rows, e.g. MCP query results) and is
//!   block-independent.
//! - [`schema`] — the fixed 6-column *block* tabular schema (a second policy
//!   over the same `toon` primitives).
//! - [`models`] — the block-forest domain types (parse-don't-validate
//!   newtypes).
//! - [`renderer`] / [`parser`] — `Forest` <-> TOON text.

pub mod error;
pub mod models;
pub mod org_reader;
pub mod parser;
pub mod renderer;
pub mod schema;
pub mod table;
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
pub use table::Row;
pub use table::Table;
pub use table::ToonValue;
