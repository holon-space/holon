//! TestPlatform-backed random GPUI PBT — Full wiring (Loro + Turso).
//!
//! Same proptest state machine and capture format as `gpui_ui_pbt`
//! but drives the window through gpui `TestPlatform` instead of a
//! real `NSWindow`.

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

fn main() {
    pbt_harness::random_pbt_sim::run(holon_pbt_core::Wiring::full(), "gpui_ui_pbt_sim");
}
