use super::*;
use crate::storage_error::IDENTITY_COLLISION_MARKER;

fn id() -> EntityUri {
    EntityUri::block("61133fe7")
}

#[test]
fn random_mints_a_prefixed_unique_block_id() {
    let a = MintedId::random();
    let b = MintedId::random();
    assert!(a.as_str().starts_with("block:"), "got {}", a.as_str());
    assert_ne!(a, b, "unique-random must not repeat");
}

#[test]
fn random_for_block_equals_block_scheme() {
    // For entity_name "block" the generic mint is the D1 block owner shape.
    let m = MintedId::random_for_entity("block");
    assert!(m.as_str().starts_with("block:"), "got {}", m.as_str());
}

#[test]
fn random_for_non_block_entity_preserves_the_entity_prefix() {
    let m = MintedId::random_for_entity("test-item");
    assert!(m.as_str().starts_with("test-item:"), "got {}", m.as_str());
}

#[test]
fn bless_carried_unheld_id_is_free() {
    let out = bless_carried(id(), None, "2026-01-15").expect("free id blesses");
    assert_eq!(out.as_entity_uri(), &id());
}

#[test]
fn bless_carried_same_normalized_title_is_idempotent() {
    // Case/space folding under normalize_for_hash: an idempotent re-create
    // of the SAME entity (AlreadySatisfied) must bless, not collide.
    let out = bless_carried(id(), Some("2026-01-15"), " 2026-01-15 ")
        .expect("already-satisfied must bless");
    assert_eq!(out.as_entity_uri(), &id());
}

#[test]
fn bless_carried_blank_holder_adopts_the_id() {
    let out = bless_carried(id(), Some(""), "Music").expect("unnamed placeholder must bless");
    assert_eq!(out.as_entity_uri(), &id());
}

#[test]
fn bless_carried_renamed_holder_is_a_collision_carrying_the_marker() {
    let err = bless_carried(id(), Some("Renamed"), "2026-01-15")
        .expect_err("different-title holder must collide");
    assert_eq!(err.id, id());
    assert_eq!(err.held_title, "Renamed");
    assert_eq!(err.requested_title, "2026-01-15");
    assert!(
        err.to_string().contains(IDENTITY_COLLISION_MARKER),
        "collision must carry the stable marker"
    );
}

#[test]
fn convergent_input_carries_the_pageid() {
    let page = PageId::for_path("Journals/2026-01-15").expect("valid path");
    match IdentityInput::convergent(&page, "2026-01-15") {
        IdentityInput::Carried { id, title } => {
            assert_eq!(&id, page.as_entity_uri());
            assert_eq!(title, "2026-01-15");
        }
        other => panic!("expected Carried, got {other:?}"),
    }
}

#[test]
fn create_id_exposes_the_underlying_id_for_both_arms() {
    let minted = CreateId::Minted(MintedId::random());
    assert!(minted.as_str().starts_with("block:"));
    let carried = CreateId::Carried(CarriedId::from_stored(id()));
    assert_eq!(carried.as_entity_uri(), &id());
}

struct NullMinter;
#[async_trait]
impl IdentityMinting for NullMinter {
    async fn mint(&self, _input: IdentityInput) -> Result<MintedId, BoxError> {
        Ok(MintedId::random())
    }
    // address_of uses the default impl.
}

#[test]
fn address_of_resolves_a_pageid_read_side_only() {
    let page = PageId::for_path("Areas/Sub").expect("valid path");
    let addr = NullMinter.address_of(&page);
    assert_eq!(addr.as_entity_uri(), page.as_entity_uri());
    assert_eq!(addr.as_str(), page.as_str());
}
