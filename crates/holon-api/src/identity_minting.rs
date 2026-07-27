//! The single minting authority (ADR 0029 D1c).
//!
//! Entity identity is an owned architectural resource: for each derivation
//! class there is exactly one sanctioned constructor, and one boundary at which
//! an id is minted. This module is the trait every consolidator's mint executor
//! implements, plus the witness types that close the caller-supplied-id back
//! door at the *type* level (what a textual lint cannot see).
//!
//! **The mode selects the mint EXECUTOR, never the id VALUE** (D1c). Derivation
//! is mode-independent by construction; a Turso-backed impl and a Loro-backed
//! impl (Inc 5) differ only in *whose store answers the recognition query and
//! records the write* — the id a given input derives to is identical on every
//! peer. So the pure decision (`bless_carried`, the derivation constructors)
//! lives here in holon-api; an impl contributes only the mode-specific store
//! read of the derived id's current holder.
//!
//! Derivation classes (D1), encoded by [`IdentityInput`]:
//! - (a) convergent-by-path — [`PageId::for_path`] / [`PageId::for_page_under`]
//!   ([`IdentityInput::convergent`]). Same inputs → same id on every peer.
//! - (b) unique-random — [`EntityUri::block_random`] ([`IdentityInput::UniqueRandom`]).
//!   Uniqueness by construction; cannot collide.
//! - (c) deterministic-by-typed-inputs — `effect_id.rs`
//!   ([`IdentityInput::deterministic`]).
//!
//! Positional derivation is a DEFECT class (D1 (d)); this module intentionally
//! offers no constructor for it.
//!
//! Witness types (ADR 0029 "Witness types complement the lint"):
//! - [`MintedId`] — the unforgeable result of a mint (its inner is private, and
//!   its only constructors are the sanctioned derivations of this module).
//! - [`CarriedId`] — an id that entered at a parse boundary via
//!   [`CarriedId::from_stored`] (a stored/param id, not freshly minted).
//! - [`CreateId`] — what a witness-typed `create` accepts: `Minted | Carried`.
//!   A bare `String`/`EntityUri` no longer typechecks into a create.
//! - [`ResolvedAddress`] — the read-side result of [`IdentityMinting::address_of`];
//!   usable for lookup / link resolution but with NO path into a [`CreateId`],
//!   so `let a = address_of(p); create(a)` does not compile.

use async_trait::async_trait;

use crate::entity_uri::EntityUri;
use crate::identity_recognition::{recognize_derived_id, Recognition};
use crate::link_parser::PageId;
use crate::storage_error::IdentityCollision;

/// The error type of [`IdentityMinting::mint`]: a store read may fail, and a
/// derived-id create may be refused with [`IdentityCollision`] (D1b). Matches
/// `holon_core::Result`'s boxed error so the create arm propagates with `?`.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

// ─────────────────────────── Witness types ───────────────────────────

/// A block id BLESSED by the minting authority: either freshly minted
/// (unique-random) or a caller-derived id that passed the D1b recognition step.
/// The inner is private and its only constructors are this module's sanctioned
/// derivations — so a `MintedId` cannot be forged from an arbitrary string.
///
/// ```compile_fail
/// use holon_api::identity_minting::MintedId;
/// use holon_api::EntityUri;
/// // The tuple field is private: a MintedId cannot be forged from a raw id.
/// let _ = MintedId(EntityUri::block("x"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintedId(EntityUri);

impl MintedId {
    /// Class (b) unique-random mint for BLOCK identity — the D1 owner
    /// ([`EntityUri::block_random`]). The single sanctioned fresh-id mint.
    pub fn random() -> Self {
        MintedId(EntityUri::block_random())
    }

    /// Class (b) unique-random mint for a NON-block entity family. The
    /// [`OperationProvider`](crate) create arm is generic over `entity_name`;
    /// ADR 0029 governs BLOCK identity (`random()` / `block_random`), so this
    /// preserves the pre-existing `{entity}:{uuid}` shape for the other
    /// families it does not govern. For `entity_name == "block"` it is
    /// identical to [`MintedId::random`]. Minter-impl use only.
    pub fn random_for_entity(entity_name: &str) -> Self {
        MintedId(EntityUri::new(entity_name, &uuid::Uuid::new_v4().to_string()))
    }

    /// Borrow the underlying id.
    pub fn as_entity_uri(&self) -> &EntityUri {
        &self.0
    }

    /// The schemed string form (`block:<id>`).
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Consume into the owned id (the write boundary hands this to persistence).
    pub fn into_entity_uri(self) -> EntityUri {
        self.0
    }
}

/// An id that entered at a parse/dispatch boundary already carrying a value — a
/// stored row id, a `create` `id` param, a split's returned id. Distinct from
/// [`MintedId`] so `create` can record *how* the id arrived (`Minted | Carried`)
/// while still refusing a bare string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarriedId(EntityUri);

impl CarriedId {
    /// The blessed parse-boundary constructor: adopt an id read from storage or
    /// supplied in op params. This is the ONE place a pre-existing id becomes a
    /// witness.
    pub fn from_stored(id: EntityUri) -> Self {
        CarriedId(id)
    }

    /// Borrow the underlying id.
    pub fn as_entity_uri(&self) -> &EntityUri {
        &self.0
    }

    /// The schemed string form.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// The id a witness-typed `create` accepts: freshly `Minted` here, or `Carried`
/// in from elsewhere. A bare `String`/`EntityUri` does not construct into this,
/// which is the point — the create boundary only takes blessed ids.
///
/// ```compile_fail
/// use holon_api::identity_minting::CreateId;
/// use holon_api::EntityUri;
/// fn create(_: CreateId) {}
/// // A bare id does not typecheck as a CreateId — the closed back door.
/// create(EntityUri::block("x"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateId {
    /// Minted at this boundary (the create-op no-id path).
    Minted(MintedId),
    /// Carried in from a caller/store (a supplied `id`, recognized for D1b).
    Carried(CarriedId),
}

impl CreateId {
    /// Borrow the underlying id regardless of provenance.
    pub fn as_entity_uri(&self) -> &EntityUri {
        match self {
            CreateId::Minted(m) => m.as_entity_uri(),
            CreateId::Carried(c) => c.as_entity_uri(),
        }
    }

    /// The schemed string form.
    pub fn as_str(&self) -> &str {
        self.as_entity_uri().as_str()
    }
}

/// The READ-side address of a name-addressable (convergent) entity: the id it
/// WOULD have, for lookup and link resolution, WITHOUT creating anything.
/// Deliberately has NO conversion into [`CreateId`] / [`MintedId`], so a derived
/// address cannot be laundered into a create — the flow a textual lint can't see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAddress(EntityUri);

impl ResolvedAddress {
    /// Borrow the resolved id (read-side only).
    pub fn as_entity_uri(&self) -> &EntityUri {
        &self.0
    }

    /// The schemed string form.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

// ─────────────────────────── Input ───────────────────────────

/// What kind of thing is entering the system — the caller states the derivation
/// intent, the authority produces the id (parse-don't-validate).
#[derive(Debug, Clone)]
pub enum IdentityInput {
    /// Class (b): mint a fresh unique-random block id. Cannot collide.
    UniqueRandom,
    /// A caller-DERIVED id carried into a create, to be recognized against its
    /// current holder before it is blessed. Covers classes (a) convergent and
    /// (c) deterministic — the DERIVATION already happened at the caller
    /// boundary ([`PageId`] / `effect_id`) and produced the id carried here; the
    /// RECOGNITION step ([`recognize_derived_id`], D1b) is identical for both.
    /// `title` is the create's content, compared against the id's holder title.
    Carried {
        /// The already-derived id.
        id: EntityUri,
        /// The requested title/content, for the recognition comparison.
        title: String,
    },
}

impl IdentityInput {
    /// Class (a) convergent-by-path: a page/journal id derived via
    /// [`PageId::for_path`] / [`PageId::for_page_under`].
    pub fn convergent(id: &PageId, title: impl Into<String>) -> Self {
        IdentityInput::Carried {
            id: id.as_entity_uri().clone(),
            title: title.into(),
        }
    }

    /// Class (c) deterministic-by-typed-inputs: an id from `effect_id.rs`.
    pub fn deterministic(id: EntityUri, title: impl Into<String>) -> Self {
        IdentityInput::Carried {
            id,
            title: title.into(),
        }
    }

    /// A carried id whose derivation class is not statically known at the
    /// generic op-dispatch boundary (a supplied `create` `id` param). The
    /// recognition step is the same regardless of class.
    pub fn carried(id: EntityUri, title: impl Into<String>) -> Self {
        IdentityInput::Carried {
            id,
            title: title.into(),
        }
    }
}

// ─────────────────────────── Pure decision ───────────────────────────

/// Complete the mint for a CARRIED (caller-derived) id given its current
/// holder's title, as read from the active store — the mode-INDEPENDENT half of
/// recognition. [`recognize_derived_id`] is the single-source predicate:
/// `Free`/`AlreadySatisfied` → the id is blessed for create; `Collision` →
/// refused (D1b interim fail-loud). `holder_title == None` ⇒ the id is unheld.
pub fn bless_carried(
    id: EntityUri,
    holder_title: Option<&str>,
    requested_title: &str,
) -> Result<MintedId, IdentityCollision> {
    match recognize_derived_id(&id, holder_title, requested_title) {
        Recognition::Free | Recognition::AlreadySatisfied => Ok(MintedId(id)),
        Recognition::Collision(c) => Err(c),
    }
}

// ─────────────────────────── Trait ───────────────────────────

/// The single minting authority. Implemented by the active consolidator's mint
/// executor (this increment: the Turso-backed [`SqlOperationProvider`](crate);
/// the Loro-backed impl is Inc 5). Reached through the `identity_minter()` DI
/// accessor, mirroring `order_key_minter`.
#[async_trait]
pub trait IdentityMinting: Send + Sync {
    /// Mint the blessed create-id for `input`.
    ///
    /// - [`IdentityInput::UniqueRandom`] → a fresh [`MintedId`]; no store read,
    ///   cannot collide.
    /// - [`IdentityInput::Carried`] → the derived id AFTER the recognition step
    ///   ([`recognize_derived_id`] via [`bless_carried`], D1b): `Free` /
    ///   `AlreadySatisfied` → `Ok`, `Collision` → `Err`([`IdentityCollision`]).
    ///
    /// This is the single minting + resolve-before-clobber entry; it subsumes
    /// the create arm's former inline pre-SELECT collision guard.
    async fn mint(&self, input: IdentityInput) -> Result<MintedId, BoxError>;

    /// Pure read-side address of a convergent (name-addressable) entity — the id
    /// it WOULD have, for link resolution, WITHOUT creating anything. Returns a
    /// [`ResolvedAddress`], which has no path into a [`CreateId`]. Mode- and
    /// self-independent (derivation is mode-independent), so a default suffices.
    fn address_of(&self, page: &PageId) -> ResolvedAddress {
        ResolvedAddress(page.as_entity_uri().clone())
    }
}

#[cfg(test)]
mod tests {
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
}
