use super::*;

#[test]
fn identity_collision_message_carries_the_marker() {
    let e = IdentityCollision {
        id: EntityUri::block("a"),
        held_title: "Renamed".into(),
        requested_title: "pagea".into(),
    };
    assert!(
        e.to_string().contains(IDENTITY_COLLISION_MARKER),
        "IdentityCollision Display must embed IDENTITY_COLLISION_MARKER so the seam survives \
             error re-wrapping; got: {e}"
    );
}
