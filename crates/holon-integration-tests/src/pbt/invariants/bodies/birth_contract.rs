//! `inv-birth-contract-satisfied` — every block an observer can see is fully
//! born: it has an identity, a place in the tree, and a place among its
//! siblings. A block visible with any of those three missing is a half-born
//! row the UI will render, sort arbitrarily, or fail to reach.
//!
//! The three facets, all read from the SUT projection (no ref):
//! - **id** — non-empty.
//! - **parentage** — `parent_id` resolves to a block in the same snapshot, or
//!   to a root sentinel (`sentinel:no_parent`). Shares [`find_orphans`] with
//!   `inv-no-orphan-blocks` so the two cannot drift on what "resolves" means.
//! - **position** — the `sort_key` the projection carries satisfies
//!   [`is_minted_key`]. The unkeyed SQL default `"A0"` is a legal column value
//!   but NOT a position (`fractional_index.rs`): the order owner re-mints such
//!   a row rather than anchoring on it, so a row still carrying it after settle
//!   means the mint never happened.
//!
//! Position needs its own cap (`SutOrderKeys`): the domain `Block` carries no
//! `sort_key` (ADR 0005), so a typed block snapshot alone cannot answer it.
//!
//! @pbt oracle internal-consistency
//! @pbt covers birth-contract-satisfied — every projected block carries an id,
//!   a resolvable parent, and a minted fractional position
//! @pbt slips-if-removed a create path publishes a block before the ordering
//!   authority mints its key; the row is visible, sorts by the unkeyed
//!   sentinel (which lexically outranks every real index), and silently
//!   reorders its siblings

use std::collections::HashMap;

use holon_api::EntityUri;
use holon_core::fractional_index::is_minted_key;
use holon_oracles::checks::ParentRow;
use holon_oracles::checks::find_orphans;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutOrderKeys;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvBirthContractSatisfied;

impl InvBirthContractSatisfied {
    pub const ID: InvariantId = InvariantId("inv-birth-contract-satisfied");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvBirthContractSatisfied
where
    S: SutBackend + SutOrderKeys,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        let blocks = sut.live_block_snapshot().await;
        let order_keys: HashMap<EntityUri, String> =
            sut.live_block_order_keys().await.into_iter().collect();

        if let Some(block) = blocks.iter().find(|b| b.id.as_str().is_empty()) {
            return InvariantResult::Fail(format!(
                "[inv-birth-contract-satisfied] block with an EMPTY id is visible in the \
                 projection (parent {}, content {:?}) — nothing can address or focus it",
                block.parent_id, block.content
            ));
        }

        if let Some(message) = find_orphans(
            &blocks
                .iter()
                .map(|b| ParentRow {
                    id: b.id.clone(),
                    parent_id: b.parent_id.clone(),
                })
                .collect::<Vec<_>>(),
        )
        .into_iter()
        .next()
        {
            return InvariantResult::Fail(format!(
                "[inv-birth-contract-satisfied] a visible block has no place in the tree: \
                 {message}"
            ));
        }

        // Every unpositioned block, not just the first. A birth-path defect is
        // usually systematic, and the blast radius (one stray row vs every doc
        // root in the vault) is the first thing the fix owner needs to know.
        let unpositioned: Vec<String> = blocks
            .iter()
            .filter_map(|block| match order_keys.get(&block.id) {
                None => Some(format!(
                    "{} (parent {}, content {:?}) carries NO order-key row",
                    block.id, block.parent_id, block.content
                )),
                Some(key) if !is_minted_key(key) => Some(format!(
                    "{} (parent {}, content {:?}) carries the unkeyed sort_key {key:?}",
                    block.id, block.parent_id, block.content
                )),
                Some(_) => None,
            })
            .collect();

        if !unpositioned.is_empty() {
            // Name a bounded sample so a systematic failure cannot bury the
            // rest of the harness output; the count carries the blast radius.
            const SHOWN: usize = 5;
            let sample = unpositioned
                .iter()
                .take(SHOWN)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ");
            let elided = unpositioned.len().saturating_sub(SHOWN);
            let tail = if elided > 0 {
                format!(" (+{elided} more)")
            } else {
                String::new()
            };
            return InvariantResult::Fail(format!(
                "[inv-birth-contract-satisfied] {} of {} visible block(s) hold no minted \
                 position — the ordering authority never minted one, so they sort arbitrarily \
                 against their siblings: {sample}{tail}",
                unpositioned.len(),
                blocks.len(),
            ));
        }

        InvariantResult::Ok
    }
}

#[cfg(test)]
mod tests {
    use holon_api::Block;
    use holon_api::EntityUri;
    use holon_pbt_core::invariant::Invariant;
    use holon_pbt_core::invariant::InvariantResult;

    use super::InvBirthContractSatisfied;

    fn uri(s: &str) -> EntityUri {
        EntityUri::parse(s).expect("valid test EntityUri")
    }

    /// A SUT double whose block snapshot and order-key column are set
    /// independently — the real APIs mint the key with the block, so only a
    /// hand-built pair can present a half-born row.
    struct HalfBornSut {
        blocks: Vec<Block>,
        order_keys: Vec<(EntityUri, String)>,
    }

    #[async_trait::async_trait(?Send)]
    impl holon_pbt_core::capabilities::SutBackend for HalfBornSut {
        async fn live_block_snapshot(&self) -> Vec<Block> {
            self.blocks.clone()
        }
        async fn block_raw_snapshot(&self) -> Vec<Block> {
            self.blocks.clone()
        }
        async fn live_focus_root_rows(&self) -> Vec<(String, String)> {
            Vec::new()
        }
    }

    #[async_trait::async_trait(?Send)]
    impl holon_pbt_core::capabilities::SutOrderKeys for HalfBornSut {
        async fn live_block_order_keys(&self) -> Vec<(EntityUri, String)> {
            self.order_keys.clone()
        }
    }

    fn failure_message(result: InvariantResult) -> String {
        match result {
            InvariantResult::Fail(message) => message,
            other => panic!("expected a birth-contract violation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn birth_contract_catches_an_unminted_sort_key() {
        let id = uri("local://unkeyed");
        let sut = HalfBornSut {
            blocks: vec![Block::new_text(
                id.clone(),
                EntityUri::no_parent(),
                "never positioned",
            )],
            // "A0" is the SQL column default: present, but not a position.
            order_keys: vec![(id.clone(), "A0".to_string())],
        };

        let message = failure_message(InvBirthContractSatisfied.check(&(), &sut).await);
        assert!(
            message.contains(id.as_str()) && message.contains("A0"),
            "the violation must name the block and the unkeyed value; got {message:?}",
        );
    }

    #[tokio::test]
    async fn birth_contract_catches_a_dangling_parent() {
        let child = uri("local://child");
        let absent_parent = uri("local://absent");
        let sut = HalfBornSut {
            blocks: vec![Block::new_text(
                child.clone(),
                absent_parent.clone(),
                "orphan",
            )],
            order_keys: vec![(child.clone(), minted_key())],
        };

        let message = failure_message(InvBirthContractSatisfied.check(&(), &sut).await);
        assert!(
            message.contains(child.as_str()) && message.contains(absent_parent.as_str()),
            "the violation must name the block and its unresolvable parent; got {message:?}",
        );
    }

    #[tokio::test]
    async fn birth_contract_catches_a_block_with_no_order_key_row() {
        let id = uri("local://keyless");
        let sut = HalfBornSut {
            blocks: vec![Block::new_text(
                id.clone(),
                EntityUri::no_parent(),
                "no order row at all",
            )],
            order_keys: Vec::new(),
        };

        let message = failure_message(InvBirthContractSatisfied.check(&(), &sut).await);
        assert!(
            message.contains(id.as_str()),
            "the violation must name the block missing an order-key row; got {message:?}",
        );
    }

    #[tokio::test]
    async fn birth_contract_passes_a_fully_born_block() {
        let id = uri("local://born");
        let sut = HalfBornSut {
            blocks: vec![Block::new_text(id.clone(), EntityUri::no_parent(), "born")],
            order_keys: vec![(id, minted_key())],
        };

        assert!(
            matches!(
                InvBirthContractSatisfied.check(&(), &sut).await,
                InvariantResult::Ok
            ),
            "a block with an id, a root parent, and a minted key satisfies the contract",
        );
    }

    /// A real generator-minted key, so the pass case cannot be an artifact of a
    /// hand-written string that happens to look minted.
    fn minted_key() -> String {
        holon_core::fractional_index::gen_n_keys(1).expect("mint one key")[0].clone()
    }
}
