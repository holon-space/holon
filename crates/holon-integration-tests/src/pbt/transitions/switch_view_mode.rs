//! Transition: switch the active view mode on a rendered ViewModeSwitcher.
//!
//! Delegates entirely to the shared `holon_pbt_core::SwitchViewMode`
//! variant + the `TransitionFactory` / `TransitionImpl` bodies in
//! `holon_layout_testing::transitions::switch_view_mode`. This file
//! exists only to satisfy the file-per-variant arch rule and to map the
//! shared `SwitchViewModeReason` into the integration-tests `Reason`
//! enum (the enum drives the rejection histogram).

use holon_layout_testing::transitions::switch_view_mode::SwitchViewModeReason;
use holon_layout_testing::{LayoutRef, LayoutSut};
use holon_pbt_core::{TransitionFactory, TransitionImpl};
use proptest::strategy::BoxedStrategy;
use validated::Validated;

pub use holon_pbt_core::SwitchViewMode;

use super::E2ETransitionImpl;
use crate::pbt::layout_bridge::SutClickAdapter;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};
use crate::pbt::validation::{Reason, map_nevec};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, REACTIVE_BASE, docs_tolerance};

fn map_reason(r: SwitchViewModeReason) -> Reason {
    match r {
        SwitchViewModeReason::NoSwitchableHandles => Reason::NoSwitchableHandles,
    }
}

impl E2ETransitionFactory for SwitchViewMode {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let ref_view = LayoutRef::new(state);
        match <SwitchViewMode as TransitionFactory<LayoutRef<'_, ReferenceState>>>::weighted_generator(&ref_view) {
            Validated::Good(x) => Validated::Good(x),
            Validated::Fail(reasons) => Validated::Fail(map_nevec(reasons, map_reason)),
        }
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for SwitchViewMode {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        if !state.app_started {
            return Validated::fail(Reason::AppNotStarted);
        }
        let ref_view = LayoutRef::new(state);
        match <SwitchViewMode as TransitionImpl<
            LayoutRef<'_, ReferenceState>,
            LayoutSut<'_, SutClickAdapter<'_>>,
        >>::preconditions(self, &ref_view)
        {
            Validated::Good(()) => Validated::Good(()),
            Validated::Fail(reasons) => Validated::Fail(map_nevec(reasons, map_reason)),
        }
    }

    fn apply_to_ref(&self, _: &mut ReferenceState) {
        // Shared variant has no ref-state model for VMS state today.
    }

    async fn apply_to_sut(&self, state: &ReferenceState, sut: &mut dyn SutHandle) {
        let ref_view = LayoutRef::new(state);
        let mut adapter = SutClickAdapter(sut);
        let mut layout_sut = LayoutSut::new(&mut adapter);
        <SwitchViewMode as TransitionImpl<
            LayoutRef<'_, ReferenceState>,
            LayoutSut<'_, SutClickAdapter<'_>>,
        >>::apply_to_sut(self, &ref_view, &mut layout_sut)
        .await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE + 10,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state) + 5,
        }
    }
}
