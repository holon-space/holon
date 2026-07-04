//! Shrink-quality certificate for the profile-driven generators.
//!
//! Deliverable-2 guard: making the generators profile-driven MUST NOT harm
//! shrinking. With a real-vault shape profile ACTIVE (so the block-count and
//! content-length bounds are widened well past the defaults), a failing
//! property must still shrink its counterexample toward the SMALL end of each
//! range — the profile only raises range upper bounds, it never reweights or
//! re-anchors the shrink target.
//!
//! Runs headless (no SUT boot), so it is fast. It sets the env var itself
//! before the first generator access; its own test binary means the
//! `active_shape_profile` `OnceLock` is fresh.

use holon_integration_tests::pbt::generators::active_shape_profile;
use holon_integration_tests::pbt::generators::blocks_per_file_gen_bound;
use holon_integration_tests::pbt::generators::content_len_gen_bound;
use proptest::prelude::*;
use proptest::test_runner::Config;
use proptest::test_runner::TestError;
use proptest::test_runner::TestRunner;

#[test]
fn profile_active_generators_still_shrink_to_small() {
    let profile_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/profiles/martin-vault-2026-07-23.json"
    );
    // SAFETY: set before any generator reads the cached profile; single-threaded
    // at this point in this dedicated test binary.
    unsafe {
        std::env::set_var("HOLON_PBT_SHAPE_PROFILE", profile_path);
    }

    let profile = active_shape_profile().expect("checked-in profile loads");
    assert_eq!(profile.schema_version, 1);
    let blocks_bound = blocks_per_file_gen_bound();
    let content_bound = content_len_gen_bound();
    assert!(
        blocks_bound > 5,
        "profile must WIDEN the block bound past the default 5 (got {blocks_bound})"
    );
    assert!(
        content_bound > 21,
        "profile must WIDEN the content bound past the default 21 (got {content_bound})"
    );

    let mut runner = TestRunner::new(Config {
        cases: 128,
        ..Config::default()
    });

    // Block-count range: a property that fails for every value must shrink to
    // the range floor 1, not to a profile-weighted middle.
    let strat = 1usize..=blocks_bound;
    let err = runner
        .run(&strat, |n| {
            prop_assert!(n < 1, "force failure for every n>=1");
            Ok(())
        })
        .expect_err("property is designed to fail");
    match err {
        TestError::Fail(_, value) => {
            assert_eq!(
                value, 1,
                "block-count must shrink to the floor (1), got {value}"
            );
        }
        other => panic!("expected a Fail, got {other:?}"),
    }

    // Content-length regex: shrinks to the shortest match (a single [A-Z]).
    let tail = content_bound.saturating_sub(1);
    let re = proptest::string::string_regex(&format!("[A-Z][a-zA-Z0-9 ]{{0,{tail}}}"))
        .expect("valid regex");
    let err = runner
        .run(&re, |s| {
            prop_assert!(s.is_empty(), "force failure for every non-empty content");
            Ok(())
        })
        .expect_err("property is designed to fail");
    match err {
        TestError::Fail(_, value) => {
            assert!(
                value.len() <= 1,
                "content must shrink to <=1 char despite the wide bound, got {value:?}"
            );
        }
        other => panic!("expected a Fail, got {other:?}"),
    }
}
