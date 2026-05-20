//! TestPlatform-backed random GPUI PBT — SqlOnly wiring (Turso, no Loro).

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

fn main() {
    pbt_harness::random_pbt_sim::run(
        holon_pbt_core::Wiring::sql_only(),
        "gpui_ui_pbt_sim_no_loro",
    );
}
