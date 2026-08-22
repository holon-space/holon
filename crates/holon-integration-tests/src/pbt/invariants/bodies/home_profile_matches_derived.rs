//! `inv-home-profile-matches-derived`: every block's capability profile, as
//! PRODUCTION resolves it from the block's home, must equal the profile the
//! DRAW implies.
//!
//! # Why the reference must not call the resolver
//!
//! The reference derives its expectation from the draw ALONE, through
//! `RefDocuments::file_home_of`: a block a tracked file holds is `org`-homed,
//! one no file holds is `holon-native`. It never calls `profile_for`.
//!
//! An oracle that asked the resolver what the answer is would compare the
//! function with itself and be green for any implementation. If a later edit
//! makes the reference side consult production, this invariant stops being
//! evidence.
//!
//! `Needs SutHomeProfile` (production's resolved home → profile id) +
//! `RefDocuments` (the draw's own document bookkeeping). Vacuously green on a
//! draw with no blocks.
//!
//! Only `HeadlessFrontendComponent` supplies `SutHomeProfile`, so this
//! deselects on every non-frontend wiring. To engage it deterministically:
//!
//! ```text
//! HOLON_PBT_FORCE_FULL=1 HOLON_PBT_WEIGHTS='WriteOrgFile:300' just keystone-smoke
//! ```
//!
//! `HOLON_PBT_FORCE_FULL=1` is what pins `full_headless`; the weights alone
//! leave the wiring drawn, and a non-frontend draw runs the whole catalog
//! without this invariant in the engagement summary.

use std::collections::HashMap;

use holon_pbt_core::capabilities::DrawnHome;
use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::RefDocuments;
use holon_pbt_core::capabilities::SutHomeProfile;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

/// The profile an org-homed block must carry.
pub const ORG: &str = "org";
/// The profile a block with no file home must carry.
pub const HOLON_NATIVE: &str = "holon-native";

pub struct InvHomeProfileMatchesDerived;

impl InvHomeProfileMatchesDerived {
    pub const ID: InvariantId = InvariantId("inv-home-profile-matches-derived");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvHomeProfileMatchesDerived
where
    R: RefDocuments,
    S: SutHomeProfile,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let observed: HashMap<String, String> = sut.home_profiles().await.into_iter().collect();

        let mut violations: Vec<String> = Vec::new();
        for (block_id, resolved) in &observed {
            // The store holds SCHEMED ids, and not all of them are blocks:
            // `sentinel:no_parent` is the FK anchor, which the draw never homes
            // and no user can address.
            if block_id.starts_with("sentinel:") {
                continue;
            }
            // Loud: an id the store holds but nobody can parse would otherwise
            // shrink the checked set without saying so.
            let uri = EntityUri::parse(block_id).unwrap_or_else(|e| {
                panic!("[{}] `{block_id}` is not an entity uri: {e}", Self::ID.0)
            });
            // THE DRAW's answer, and nothing else: does a tracked file hold
            // this block?
            let expected = match ref_.file_home_of(&uri) {
                DrawnHome::File(_) => ORG,
                DrawnHome::Storeless => HOLON_NATIVE,
                DrawnHome::Unmodelled => continue,
            };
            if resolved != expected {
                violations.push(format!(
                    "block `{block_id}`: home resolves to `{resolved}` but the draw homes it in \
                     `{expected}`"
                ));
            }
        }

        if violations.is_empty() {
            return InvariantResult::Ok;
        }
        violations.sort();
        InvariantResult::Fail(format!(
            "[inv-home-profile-matches-derived] {} block(s) carry a profile their home does not \
             imply: {:?}",
            violations.len(),
            violations.iter().take(10).collect::<Vec<_>>(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reference that homes exactly the blocks it was told to — the DRAW,
    /// with no access to the resolver.
    struct DrawRef {
        homed: Vec<String>,
        unmodelled: Vec<String>,
    }

    impl RefDocuments for DrawRef {
        fn document_names(&self) -> Vec<String> {
            Vec::new()
        }
        fn has_document(&self, _: &str) -> bool {
            false
        }
        fn document_count(&self) -> usize {
            0
        }
        fn doc_uri_by_name(&self, _: &str) -> Option<EntityUri> {
            None
        }
        fn block_document_of(&self, _: &EntityUri) -> Option<EntityUri> {
            None
        }
        fn has_non_seed_advice_rule(&self) -> bool {
            false
        }
        fn document_uris(&self) -> Vec<EntityUri> {
            Vec::new()
        }
        fn has_document_uri(&self, _: &EntityUri) -> bool {
            false
        }
        fn file_home_of(&self, block_id: &EntityUri) -> DrawnHome {
            if self.homed.iter().any(|b| b == block_id.id()) {
                return DrawnHome::File(EntityUri::block("doc"));
            }
            if self.unmodelled.iter().any(|b| b == block_id.id()) {
                return DrawnHome::Unmodelled;
            }
            DrawnHome::Storeless
        }
    }

    /// A SUT reporting whatever it is told the resolver said.
    struct SutStub {
        rows: Vec<(String, String)>,
    }

    #[async_trait::async_trait(?Send)]
    impl SutHomeProfile for SutStub {
        async fn home_profiles(&self) -> Vec<(String, String)> {
            self.rows.clone()
        }
    }

    fn check(homed: &[&str], rows: &[(&str, &str)]) -> InvariantResult {
        check_with(homed, &[], rows)
    }

    fn check_with(homed: &[&str], unmodelled: &[&str], rows: &[(&str, &str)]) -> InvariantResult {
        let ref_ = DrawRef {
            homed: homed.iter().map(|s| s.to_string()).collect(),
            unmodelled: unmodelled.iter().map(|s| s.to_string()).collect(),
        };
        let sut = SutStub {
            rows: rows
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
        };
        futures::executor::block_on(InvHomeProfileMatchesDerived.check(&ref_, &sut))
    }

    /// A resolver answering `holon-native` for a block the draw homed in an
    /// org file is what this invariant exists to catch.
    #[test]
    fn an_org_homed_block_resolving_to_holon_native_fails() {
        let result = check(&["a"], &[("block:a", HOLON_NATIVE)]);
        let InvariantResult::Fail(message) = result else {
            panic!("the stub must NOT satisfy an org-homed draw: {result:?}");
        };
        assert!(
            message.contains("`block:a`") && message.contains(ORG),
            "the failure must name the block and the profile the draw implies: {message}"
        );
    }

    /// The other side: a block the draw did NOT home is holon-native, and the
    /// stub happens to be right. Without this the red above would prove only
    /// that the invariant fails on everything.
    #[test]
    fn a_block_with_no_file_home_is_holon_native() {
        assert!(matches!(
            check(&[], &[("block:a", HOLON_NATIVE)]),
            InvariantResult::Ok
        ));
    }

    /// And the true answer passes, so the invariant is not simply anti-`org`.
    #[test]
    fn an_org_homed_block_resolving_to_org_is_green() {
        assert!(matches!(
            check(&["a"], &[("block:a", ORG)]),
            InvariantResult::Ok
        ));
    }

    /// A block the draw never modeled carries no expectation, so NEITHER
    /// profile can fail on it.
    #[test]
    fn a_block_the_draw_does_not_model_is_skipped() {
        for resolved in [ORG, HOLON_NATIVE] {
            assert!(matches!(
                check_with(&[], &["a"], &[("block:a", resolved)]),
                InvariantResult::Ok
            ));
        }
    }
}
