//! Shared `DeliverBlockContent` semantics: pick a block id from the
//! ref-state's deferred-block set; push its loaded content through the
//! frontend's `LiveBlockSink`. Not a user action — a test stimulus.

use holon_pbt_core::DeliverBlockContent;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionImpl;
use holon_pbt_core::TransitionRef;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated::Good;
use validated::Validated::{self};

use crate::sut::LayoutRef;
use crate::sut::LayoutRefState;
use crate::sut::LayoutSut;
use crate::sut::LiveBlockSink;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DeliverBlockContentReason {
    NoDeferredBlocks,
}

impl<R> TransitionFactory<LayoutRef<'_, R>> for DeliverBlockContent
where
    R: LayoutRefState + ?Sized,
{
    type Reason = DeliverBlockContentReason;
    fn weighted_generator(
        state: &LayoutRef<'_, R>,
    ) -> Validated<(u32, BoxedStrategy<Self>), Self::Reason> {
        let ids = state.deferred_block_ids();
        if ids.is_empty() {
            return Validated::fail(DeliverBlockContentReason::NoDeferredBlocks);
        }
        let strat = proptest::sample::select(ids)
            .prop_map(|block_id| DeliverBlockContent { block_id })
            .boxed();
        Good((1, strat))
    }
}

impl<R> TransitionRef<LayoutRef<'_, R>> for DeliverBlockContent
where
    R: LayoutRefState + ?Sized,
{
    type Reason = DeliverBlockContentReason;
    fn preconditions(&self, _: &LayoutRef<'_, R>) -> Validated<(), Self::Reason> {
        Good(())
    }
    fn apply_to_ref(&self, _: &mut LayoutRef<'_, R>) {}
}

impl<R, S> TransitionImpl<LayoutRef<'_, R>, LayoutSut<'_, S>> for DeliverBlockContent
where
    R: LayoutRefState + ?Sized,
    S: LiveBlockSink + ?Sized,
{
    async fn apply_to_sut(&self, _: &LayoutRef<'_, R>, sut: &mut LayoutSut<'_, S>) {
        sut.deliver_block_content_loaded(&self.block_id);
    }
}
