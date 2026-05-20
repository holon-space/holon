//! GPUI UI PBT (SqlOnly wiring — Loro disabled) — the no-Loro twin of
//! `gpui_ui_pbt`: Turso storage, no MutableText/cell, editor content
//! persisted only by the on-blur `set_field`. Its own test target so the
//! no-Loro configuration runs automatically instead of hiding behind an
//! env var. Shared body: [`pbt_harness::random_pbt`]; harness docs in
//! `gpui_ui_pbt.rs`.
//!
//! `harness = false` (GPUI needs the main thread). Run with:
//!   cargo test -p holon-gpui --test gpui_ui_pbt_no_loro --features pbt

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

fn main() {
    pbt_harness::random_pbt::run(holon_pbt_core::Wiring::sql_only(), "gpui_ui_pbt_no_loro");
}
