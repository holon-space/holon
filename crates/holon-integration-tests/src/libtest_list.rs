//! The libtest `--list` collection protocol, for `harness = false` test
//! binaries whose body boots a real window / full SUT.
//!
//! Enrolled: `gpui_gherkin_replay`, `gpui_sim_replay_capture`,
//! `gpui_capture_replay`, `gpui_windowed_minimize`, `tui_ui_pbt`,
//! `executor_bridge_test`.
//!
//! DEFERRED, ruled by Martin 2026-08-08: `reactive_vm_test` (~40 sub-tests) and
//! `reactive_vm_realwindow_test` stay unenrolled. Enrolling a binary makes it a
//! member of a plain `cargo nextest run`, and their combined scenario wall time
//! is not worth paying yet. Collection survives without them only because their
//! stdout stays empty during the probe, so nextest reads them as zero tests
//! rather than as a corrupt listing — the scenario still executes at `--list`.

/// Answer a test runner's `--list` collection probe, so a `harness = false`
/// binary can be ENUMERATED without booting its scenario.
///
/// `cargo nextest` collects by running every test binary twice with
/// `--list --format terse` (once plain, once `--ignored`) and requires each
/// printed line to end in `: test`; a binary that ignores the flag instead
/// runs its whole scenario during collection and aborts the run. Call this as
/// the FIRST statement of `main()` and return immediately when it returns
/// `true` — nothing may write to stdout before it.
#[must_use]
pub fn handled_list_protocol(test_name: &str) -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|a| a == "--list") {
        return false;
    }
    // `--ignored` asks for the ignored-only subset; these binaries have none.
    if !args.iter().any(|a| a == "--ignored") {
        println!("{test_name}: test");
    }
    true
}
