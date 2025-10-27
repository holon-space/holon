//! Cross-frontend PBT — headless FFI-only test.
//!
//! Validates the phased PBT state machine without any frontend process.
//! All UI operations fall back to direct FFI execution via `pbt_execute_operation`.

use holon_integration_tests::pbt::phased::run_phased_pbt_sync;

#[test]
fn headless_phased_pbt() {
    match run_phased_pbt_sync(15) {
        Ok(summary) => eprintln!("[cross_frontend_pbt] {summary}"),
        Err(e) => panic!("Cross-frontend PBT failed: {e:?}"),
    }
}
