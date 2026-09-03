//! JSON Lines of typed rows — the neutral contract between whatever produced
//! rows (a format plugin, a remote system's response) and the one sink that
//! writes them ([`holon_core::file_format::TypedRowSink`]).
//!
//! Pure serde: no I/O, no clock, no filesystem, so the same code runs natively,
//! in a wasm guest and in the web worker.

mod emit;
mod envelope;
mod ids;
mod mapping;
mod parse;

pub use emit::emit_row_sets;
pub use envelope::CONTRACT_VERSION;
pub use envelope::Envelope;
pub use envelope::ScopeHeader;
pub use ids::checked_local_id;
pub use mapping::RowMapper;
pub use parse::parse_row_sets;
