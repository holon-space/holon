//! TUI composed windowed random runner (increment 4c) — the TUI sibling of the
//! gpui `gpui_composed_windowed_loop`: the wide-seeded `ComposedSut<WideE2E>`
//! base rendered by the TUI capturing renderer, gestures driven through the
//! real `TuiUserDriver`, ONE generated `E2ETransition` sequence from the
//! windowed alphabet, the full composed catalog checked every tick.
//!
//! Shared body: `common::pbt_main` (architecture + disclosed deviations from
//! the gpui loop are documented there).
//!
//! `harness = false`. Run with: `cargo test -p holon-tui --test tui_ui_pbt`.
//!
//! ## Environment variables
//!
//! - `PROPTEST_SEED=<u64>` — pin the random seed (proptest standard); the
//!   deterministic-reproduction knob (no in-process shrinking here — the
//!   shrinker home is the gpui loop / headless keystone).
//! - `PBT_NUM_STEPS=<n>` — sequence length upper bound (default 8).
//! - `HOLON_PBT_WEIGHTS=Indent:200,Outdent:200,Move*:50` — bias the strategy
//!   aggregator toward (or away from) named transition variants. See
//!   `crates/holon-integration-tests/src/pbt/transition_dispatch.rs`.

mod common;

fn main() {
    if holon_integration_tests::libtest_list::handled_list_protocol("tui_ui_pbt") {
        return;
    }
    common::pbt_main::run("tui_ui_pbt");
}
