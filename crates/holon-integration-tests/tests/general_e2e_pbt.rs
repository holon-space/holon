//! General-Purpose Property-Based E2E Test
//!
//! This is the test entry point. The state machine, SUT, generators, and types
//! live in `src/pbt/` so they can be reused by other harnesses (e.g. Flutter FFI).

use proptest::prelude::*;

use holon_integration_tests::pbt::{E2ESut, Full, SqlOnly};

fn pbt_config() -> ProptestConfig {
    let max_shrink = std::env::var("PROPTEST_MAX_SHRINK_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    ProptestConfig {
        cases: 8,
        max_shrink_iters: max_shrink,
        ..ProptestConfig::default()
    }
}

proptest_state_machine::prop_state_machine! {
    #![proptest_config(pbt_config())]

    #[test]
    fn general_e2e_pbt(sequential 3..20 => E2ESut<Full>);
}

proptest_state_machine::prop_state_machine! {
    #![proptest_config(pbt_config())]

    #[test]
    fn general_e2e_pbt_sql_only(sequential 3..20 => E2ESut<SqlOnly>);
}
