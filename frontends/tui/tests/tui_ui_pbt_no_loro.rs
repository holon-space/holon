//! TUI UI PBT (SqlOnly wiring — Loro disabled) — the no-Loro twin of
//! `tui_ui_pbt`: Turso storage, no MutableText/cell, editor content
//! persisted only by the on-blur `set_field`. Its own test target so the
//! no-Loro configuration runs automatically instead of hiding behind an
//! env var. Shared body: `common::pbt_main`; harness docs in
//! `tui_ui_pbt.rs`.
//!
//! `harness = false`. Run with:
//!   cargo test -p holon-tui --test tui_ui_pbt_no_loro

mod common;

fn main() {
    common::pbt_main::run(holon_pbt_core::Wiring::sql_only(), "tui_ui_pbt_no_loro");
}
