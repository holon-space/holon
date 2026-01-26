//! Shared PBT entry point for Flutter integration tests.
//!
//! Two modes:
//! 1. **Monolithic** (`run_shared_pbt`): FFI-only, Dart callback executes operations directly.
//! 2. **Phased** (`pbt_setup`/`pbt_step`/`pbt_teardown`): re-exported from shared crate, but
//!    Flutter wraps `pbt_setup` and `pbt_teardown` to install/clear the PBT engine as GLOBAL_SESSION.

use flutter_rust_bridge::DartFnFuture;
use std::sync::Arc;

use crate::api::ffi_bridge::{clear_pbt_engine, set_pbt_engine};
use crate::api::flutter_mutation_driver::{ApplyMutationCallback, FlutterMutationDriver};
use holon_integration_tests::pbt::types::Full;
use holon_integration_tests::pbt::{E2ESut, E2ETransition, VariantRef};
use holon_integration_tests::MutationDriver;
use proptest::prelude::*;
use proptest_state_machine::ReferenceStateMachine;

// Re-export phased PBT types from the shared crate
pub use holon_integration_tests::pbt::phased::{PbtStepResult, PbtUiOperation};

// Re-export phased functions that don't need Flutter-specific wiring
pub use holon_integration_tests::pbt::phased::{pbt_step, pbt_step_confirm};

use holon_integration_tests::pbt::phased::{
    create_initial_ref_state, create_runner, create_runtime,
};

// ──── Shared helpers ────

fn generate_transition(
    runner: &mut proptest::test_runner::TestRunner,
    ref_state: &VariantRef<Full>,
    step: u32,
) -> anyhow::Result<Option<E2ETransition>> {
    let strategy = <VariantRef<Full> as ReferenceStateMachine>::transitions(ref_state);
    let transition = strategy
        .new_tree(runner)
        .map_err(|e| anyhow::anyhow!("Failed to generate transition at step {step}: {e}"))?
        .current();

    if !<VariantRef<Full> as ReferenceStateMachine>::preconditions(ref_state, &transition) {
        return Ok(None);
    }

    Ok(Some(transition))
}

// ──── Mode 1: Monolithic (FFI-only, existing) ────

/// Run the full PBT state machine with Flutter providing UI mutations via callback.
pub async fn run_shared_pbt(
    apply_mutation_cb: impl Fn(String, String, String) -> DartFnFuture<()> + Send + Sync + 'static,
    num_steps: u32,
) -> anyhow::Result<String> {
    let driver: Box<dyn MutationDriver> = Box::new(FlutterMutationDriver::new(Arc::new(
        apply_mutation_cb,
    )
        as ApplyMutationCallback));

    let runtime = create_runtime();
    let mut sut = E2ESut::<Full>::with_driver(runtime, driver)?;
    let mut runner = create_runner()?;
    let mut ref_state = create_initial_ref_state(&mut runner)?;

    let mut actual_steps = 0u32;
    let mut pbt_engine_installed = false;

    for step in 0..num_steps {
        let transition = match generate_transition(&mut runner, &ref_state, step)? {
            Some(t) => t,
            None => continue,
        };

        let is_start_app = matches!(&transition, E2ETransition::StartApp { .. });

        ref_state = <VariantRef<Full> as ReferenceStateMachine>::apply(ref_state, &transition);
        sut.apply_transition_async(&ref_state, &transition).await;

        if is_start_app && !pbt_engine_installed {
            set_pbt_engine(sut.engine().clone());
            pbt_engine_installed = true;
        }

        sut.check_invariants_async(&ref_state).await;
        actual_steps += 1;

        eprintln!(
            "[run_shared_pbt] Step {}/{}: {:?} ✓",
            step + 1,
            num_steps,
            std::mem::discriminant(&transition)
        );
    }

    if pbt_engine_installed {
        clear_pbt_engine();
    }

    std::thread::spawn(move || {
        drop(sut);
        drop(ref_state);
    })
    .join()
    .expect("SUT cleanup thread panicked");

    Ok(format!(
        "passed: {actual_steps}/{num_steps} PBT transitions"
    ))
}

// ──── Mode 2: Flutter-specific phased wrappers ────

/// Flutter-specific pbt_setup that also installs the PBT engine as GLOBAL_SESSION.
pub async fn pbt_setup(num_steps: u32) -> anyhow::Result<String> {
    use crate::api::ffi_bridge::install_pbt_as_global_session;

    let runtime = create_runtime();
    let mut sut = E2ESut::<Full>::new(runtime)?;
    let mut runner = create_runner()?;
    let mut ref_state = create_initial_ref_state(&mut runner)?;

    let mut current_step = 0u32;
    let mut actual_steps = 0u32;

    let mut start_app_done = false;
    while current_step < num_steps && !start_app_done {
        let transition = match generate_transition(&mut runner, &ref_state, current_step)? {
            Some(t) => t,
            None => {
                current_step += 1;
                continue;
            }
        };

        let is_start_app = matches!(&transition, E2ETransition::StartApp { .. });

        ref_state = <VariantRef<Full> as ReferenceStateMachine>::apply(ref_state, &transition);
        sut.apply_transition_async(&ref_state, &transition).await;

        if is_start_app {
            set_pbt_engine(sut.engine().clone());
            install_pbt_as_global_session()?;
            start_app_done = true;
        }

        sut.check_invariants_async(&ref_state).await;
        actual_steps += 1;
        current_step += 1;

        eprintln!(
            "[pbt_setup] Step {}/{}: {:?} ✓",
            current_step,
            num_steps,
            std::mem::discriminant(&transition)
        );
    }

    assert!(
        start_app_done,
        "pbt_setup exhausted all steps without reaching StartApp"
    );

    sut.driver = Some(Box::new(
        holon_integration_tests::DirectMutationDriver::new(sut.ctx.engine().clone()),
    ));

    let summary = format!("setup complete: {actual_steps} pre-startup steps");

    // Store state in the shared crate's PBT_PHASE_STATE
    holon_integration_tests::pbt::phased::store_phase_state(
        holon_integration_tests::pbt::phased::PbtPhaseState {
            sut,
            ref_state,
            runner,
            num_steps,
            current_step,
            actual_steps,
        },
    );

    Ok(summary)
}

/// Flutter-specific pbt_teardown that also clears the PBT engine.
pub async fn pbt_teardown() -> anyhow::Result<String> {
    let result = holon_integration_tests::pbt::phased::pbt_teardown().await;
    clear_pbt_engine();
    result
}
