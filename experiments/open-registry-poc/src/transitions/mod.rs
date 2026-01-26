//! Each transition lives in its own module and self-registers. `mod.rs` only
//! has to *include* the modules so their `inventory::submit!` constructors are
//! linked — it does not enumerate or dispatch anything. (A build script that
//! globbed this directory could even remove this last vestige of
//! centralization.)

pub mod split;
pub mod toggle;
pub mod typechar;
