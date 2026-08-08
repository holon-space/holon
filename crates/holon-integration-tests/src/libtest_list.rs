//! The libtest `--list` collection protocol, for `harness = false` test
//! binaries whose body boots a real window / full SUT.

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
