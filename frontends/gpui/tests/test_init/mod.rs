//! Observability init for every windowed (`frontends/gpui/tests/*`) test
//! binary.
//!
//! Windowed targets used to install NO tracing subscriber at all, so every
//! `warn!`/`error!` a widget, driver or background worker emitted during a test
//! was discarded by `tracing`'s no-op global dispatcher before anything could
//! read it. That made the "falls back VISIBLY" tier of the error philosophy
//! unenforceable here: a degradation could announce itself perfectly and the
//! windowed suite would still be green and silent (a silent-empty context-path
//! resolution once survived six rounds of #27 this way).
//!
//! Declaring `mod test_init;` in a test file is the whole wiring — the
//! constructor below runs before `main`/the libtest harness, so the subscriber
//! is up before any SUT code can emit. `windowed_log_capture.rs` fails the
//! suite if a target forgets the declaration.

#![allow(dead_code)] // each binary reads a different subset

use holon_integration_tests::test_tracing;
pub use holon_integration_tests::test_tracing::CapturedProblem;

/// Install the process-global capturing subscriber before any test code runs.
///
/// Idempotence is owned by `SpanCollector::global()`'s `OnceLock`, not by a
/// `try_init` that would swallow a competing installer: this constructor runs
/// exactly once per binary, so `set_global_default` inside it is reached
/// exactly once and a genuine double-install still panics loudly there.
#[ctor::ctor]
fn install_capturing_subscriber() {
    test_tracing::SpanCollector::global();
}

/// Open a fresh capture window owned by the calling test thread, retiring the
/// one it owned before. Call at the start of any test that reads
/// [`captured_warnings`] or [`captured_problems`] — a read from a thread with
/// no window panics rather than silently returning nothing.
pub fn begin_case() {
    test_tracing::begin_test_scope();
}

/// WARN events captured in this thread's window — the disclosed-degradation
/// tier. Assertable on demand; never fatal by itself.
pub fn captured_warnings() -> Vec<CapturedProblem> {
    test_tracing::SpanCollector::global().captured_warnings()
}

/// ERROR events and swallowed panics captured in this thread's window — the
/// same set `inv-no-observed-errors` reds on in the keystone.
pub fn captured_problems() -> Vec<CapturedProblem> {
    test_tracing::SpanCollector::global().captured_problems()
}
