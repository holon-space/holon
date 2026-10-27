use super::*;

#[test]
fn detect_maps_loro_presence_to_profile() {
    assert_eq!(
        CapabilityProfile::detect(true),
        CapabilityProfile::Projected
    );
    assert_eq!(CapabilityProfile::detect(false), CapabilityProfile::Direct);
}

#[test]
fn projected_owns_upstream_and_has_downstream() {
    let p = CapabilityProfile::Projected;
    assert!(p.has_downstream_projection());
    assert_eq!(p.consolidator(), Consolidator::Upstream);
}

#[test]
fn direct_owns_via_store_and_has_no_downstream() {
    let p = CapabilityProfile::Direct;
    assert!(!p.has_downstream_projection());
    assert_eq!(p.consolidator(), Consolidator::Store);
}

#[test]
fn pinned_caps_carry_consolidator() {
    let caps = SessionCapabilities::detect_and_pin(true);
    assert_eq!(caps.profile(), CapabilityProfile::Projected);
    assert_eq!(caps.consolidator(), Consolidator::Upstream);

    let degraded = SessionCapabilities::detect_and_pin(false);
    assert_eq!(degraded.consolidator(), Consolidator::Store);
}
