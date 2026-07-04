//! The Loro subsystem's composed-keystone invariants: body + `wire()` per file.
//! Bodies read only `holon-pbt-core` capability traits (`SutLoroLog`,
//! `RefBlockTree`), so they carry no dependency on the central reference state.

pub mod loro_blocks_match_ref;
pub mod loro_children_match_ref;
pub mod loro_no_errors;
