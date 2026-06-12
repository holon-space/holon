//! Deterministic sim (TestPlatform) replay of a single captured failing
//! sequence — a LOCKED, debuggable reproduction (no proptest generation, so the
//! exact same steps run every time). Used to pin the `PressKey`→Loro windowed
//! divergence (`presskey_loro_split_backspace.json`): split "Q8" at 1, then at 0
//! twice, then Backspace — the reference keeps "8" but the SUT loses it.
//!
//! `harness = false` (GPUI needs the main thread). Run with:
//!   cargo test -p holon-gpui --test gpui_sim_replay_capture --features pbt
//! Override the capture with `HOLON_CAPTURE=/abs/path.json`.

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

fn main() {
    pbt_harness::random_pbt_sim::replay_capture(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/presskey_loro_split_backspace.json"
    ));
}
