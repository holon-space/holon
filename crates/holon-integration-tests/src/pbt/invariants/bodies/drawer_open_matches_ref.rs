//! `inv-drawer-open-matches-ref`.
//!
//! @pbt oracle correspondence
//! @pbt covers drawer-open-snapshot — the rendered drawer's open/closed state
//!   disagrees with the reference's `drawer_is_open`, or is absent from the
//!   snapshot entirely
//! @pbt slips-if-removed a frontend that renders from the snapshot (rather than
//!   reading the live view-store, as GPUI does) paints every drawer open — a
//!   closed sidebar keeps its full width and stays visible
//!
//! Expressed against the frontend-agnostic `WidgetSnapshot` IR so it runs in
//! any slice whose SUT implements `SutRenderer`.
//!
//! Asserts, for every `drawer` widget in the snapshot: the `open` prop is
//! present and parses as a bool, and equals `RefNavHistory::drawer_is_open`
//! for the drawer's `block_id`. Absence is a failure, not a skip — a drawer
//! node that carries no open state cannot be rendered collapsed.

use holon_pbt_core::capabilities::RefNavHistory;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvDrawerOpenMatchesRef;

impl InvDrawerOpenMatchesRef {
    pub const ID: InvariantId = InvariantId("inv-drawer-open-matches-ref");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvDrawerOpenMatchesRef
where
    R: RefNavHistory,
    S: SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let root = sut.widget_tree_snapshot().await;
        for node in root.walk() {
            if node.kind != "drawer" {
                continue;
            }

            let Some(block_id) = node.props.get("block_id") else {
                return InvariantResult::Fail(
                    "[inv-drawer-open-matches-ref] drawer node carries no 'block_id' prop".into(),
                );
            };

            let Some(open) = node.props.get("open") else {
                return InvariantResult::Fail(format!(
                    "[inv-drawer-open-matches-ref] drawer {block_id} carries no 'open' prop — the \
                     snapshot has no open/closed state, so a snapshot-driven frontend must render \
                     it open (props: {:?})",
                    node.props
                ));
            };
            let Ok(open) = open.parse::<bool>() else {
                return InvariantResult::Fail(format!(
                    "[inv-drawer-open-matches-ref] drawer {block_id} 'open' prop is not a bool: \
                     '{open}'"
                ));
            };

            let expected = ref_.drawer_is_open(block_id);
            if open != expected {
                return InvariantResult::Fail(format!(
                    "[inv-drawer-open-matches-ref] drawer {block_id} rendered open={open} but \
                     reference says open={expected}"
                ));
            }
        }
        InvariantResult::Ok
    }
}
