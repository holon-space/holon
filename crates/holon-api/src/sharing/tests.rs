use super::*;

#[test]
fn read_only_does_not_confer_write() {
    assert!(!Capabilities::read_only().contains(Capability::Write));
    assert!(Capabilities::read_write().contains(Capability::Write));
}

#[test]
fn intersect_never_grows_a_grant() {
    let held = Capabilities::read_only();
    let asked = Capabilities::read_write();
    assert_eq!(held.intersect(&asked), Capabilities::read_only());
    assert!(held.intersect(&asked).is_subset_of(&held));
}

#[test]
fn a_refusal_can_name_the_set_it_refused() {
    assert_eq!(Capabilities::read_write().to_string(), "{read, write}");
    assert_eq!(Capabilities::default().to_string(), "{}");
}
