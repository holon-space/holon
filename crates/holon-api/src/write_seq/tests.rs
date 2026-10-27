use super::*;

#[test]
fn next_is_strictly_monotonic_and_above_zero() {
    let a = next();
    let b = next();
    let c = next();
    assert!(
        a > WriteSeq::ZERO,
        "first allocation must exceed the row default"
    );
    assert!(b > a && c > b, "sequences must be strictly increasing");
}

#[test]
fn round_trips_through_i64() {
    let s = next();
    assert_eq!(WriteSeq::from_i64(s.get()), s);
}
