//! `ConditionProfile::new` refuses two combinations. Every profile in the crate
//! is a `const`, so in practice the refusal lands as `E0080` at compile time
//! and no runtime test can ever see it. These tests pin the runtime behaviour
//! of the constructor itself, so the guard is still exercised if a future
//! profile is ever built from a non-const value.

use std::time::Duration;

use holon_api::AllClear;
use holon_api::ConditionPlacement;
use holon_api::ConditionProfile;
use holon_api::ConditionSeverity;
use holon_api::OpId;
use holon_api::RemedySlot;
use holon_api::SettingsSection;

const LABEL: &str = "test condition";
const ICON: &str = holon_api::condition_profile::icons::WARN;
const PLACEMENT: ConditionPlacement = ConditionPlacement::Banner;

#[test]
#[should_panic(expected = "AllClear::Elapsed is legal only for ConditionSeverity::Info")]
fn elapsed_is_refused_for_warning() {
    let _ = ConditionProfile::new(
        ConditionSeverity::Warning,
        LABEL,
        ICON,
        PLACEMENT,
        AllClear::Elapsed(Duration::from_secs(5)),
        &[],
    );
}

#[test]
#[should_panic(expected = "AllClear::Elapsed is legal only for ConditionSeverity::Info")]
fn elapsed_is_refused_for_error() {
    let _ = ConditionProfile::new(
        ConditionSeverity::Error,
        LABEL,
        ICON,
        PLACEMENT,
        AllClear::Elapsed(Duration::from_secs(5)),
        &[],
    );
}

#[test]
#[should_panic(expected = "RemedySlot::Dismiss is legal only where AllClear is RemedyApplied")]
fn dismiss_is_refused_for_until_restart() {
    let _ = ConditionProfile::new(
        ConditionSeverity::Info,
        LABEL,
        ICON,
        PLACEMENT,
        AllClear::UntilRestart,
        &[RemedySlot::Dismiss],
    );
}

#[test]
#[should_panic(expected = "RemedySlot::Dismiss is legal only where AllClear is RemedyApplied")]
fn dismiss_is_refused_for_next_clean_ingest() {
    let _ = ConditionProfile::new(
        ConditionSeverity::Info,
        LABEL,
        ICON,
        PLACEMENT,
        AllClear::NextCleanIngest,
        &[
            RemedySlot::Retry(OpId::PairRetryReimport),
            RemedySlot::Dismiss,
        ],
    );
}

#[test]
fn dismiss_is_accepted_where_the_remedy_resolves_it() {
    let profile = ConditionProfile::new(
        ConditionSeverity::Error,
        LABEL,
        ICON,
        PLACEMENT,
        AllClear::RemedyApplied,
        &[
            RemedySlot::Retry(OpId::PairRetryReimport),
            RemedySlot::Dismiss,
        ],
    );
    assert_eq!(
        profile.remedies(),
        &[
            RemedySlot::Retry(OpId::PairRetryReimport),
            RemedySlot::Dismiss
        ]
    );
}

#[test]
fn elapsed_is_accepted_for_info() {
    let profile = ConditionProfile::new(
        ConditionSeverity::Info,
        LABEL,
        ICON,
        PLACEMENT,
        AllClear::Elapsed(Duration::from_secs(5)),
        &[RemedySlot::Dismiss],
    );
    assert_eq!(profile.severity(), ConditionSeverity::Info);
}

#[test]
fn a_placement_section_carries_its_section() {
    let profile = ConditionProfile::new(
        ConditionSeverity::Warning,
        LABEL,
        ICON,
        ConditionPlacement::Section(SettingsSection::Integrations),
        AllClear::UntilRestart,
        &[],
    );
    assert_eq!(
        profile.placement(),
        ConditionPlacement::Section(SettingsSection::Integrations)
    );
}
