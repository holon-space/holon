//! Phased PBT API — setup/step/confirm/teardown cycle for cross-frontend testing.
//!
//! Extracted from `frontends/flutter/rust/src/api/shared_pbt.rs` so any frontend
//! (or a headless test) can reuse the same state machine.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use holon_api::{EntityUri, Value};
use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use proptest_state_machine::ReferenceStateMachine;

use super::types::{Full, MutationSource};
use super::{E2ESut, E2ETransition, VariantRef};
use crate::DirectUserDriver;

// ──── Public types ────

/// Context provided to the `on_ready` callback after StartApp completes.
/// Contains everything needed to launch a frontend window sharing the PBT's state.
/// Context provided to the `on_ready` callback after StartApp completes.
pub struct PbtReadyContext {
    pub engine: Arc<holon::api::BackendEngine>,
    pub session: Arc<holon_frontend::FrontendSession>,
    pub reactive_engine: Arc<holon_frontend::reactive::ReactiveEngine>,
    pub runtime_handle: tokio::runtime::Handle,
}

/// Result returned by the `on_ready` callback.
pub struct PbtReadyResult {
    /// Custom mutation driver (None = use DirectUserDriver).
    pub driver: Option<Box<dyn crate::UserDriver>>,
    /// Optional frontend ReactiveEngine for inv14 assertions.
    /// When set, each transition checks the frontend's ViewModel for errors.
    pub frontend_engine: Option<Arc<holon_frontend::reactive::ReactiveEngine>>,
    /// Optional geometry provider for inv14 bounds assertions.
    /// When set, checks that GPUI actually laid out the expected elements.
    pub frontend_geometry: Option<Box<dyn holon_frontend::geometry::GeometryProvider>>,
    /// Optional shared screenshot analysis state for inv14 empty-UI detection.
    pub frontend_visual_state: Option<crate::ui_driver::VisualState>,
}

/// Result of a single PBT step.
pub struct PbtStepResult {
    /// True when all steps are exhausted.
    pub done: bool,
    /// Human-readable transition name (for logging).
    pub transition_name: &'static str,
    /// If set, this is a UI mutation the caller should try to execute.
    /// If unhandled, fall back to FFI via `pbt_execute_operation`.
    pub ui_operation: Option<PbtUiOperation>,
}

/// A UI mutation the caller should attempt via the widget tree.
pub struct PbtUiOperation {
    /// Entity name (e.g. "block")
    pub entity: String,
    /// Operation name (e.g. "set_field", "create", "delete")
    pub op: String,
    /// JSON-serialized HashMap<String, Value> parameters
    pub params_json: String,
    /// Pre-resolved parameters (for direct FFI use without re-parsing JSON)
    pub params: HashMap<String, Value>,
}

// ──── Shared helpers ────

/// Generate the next transition from the reference state using proptest.
///
/// Sync function so non-Send `BoxedStrategy` doesn't live across `.await`.
fn generate_transition(
    runner: &mut TestRunner,
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

pub fn create_runtime() -> Arc<tokio::runtime::Runtime> {
    std::thread::spawn(|| {
        Arc::new(tokio::runtime::Runtime::new().expect("Failed to create PBT tokio runtime"))
    })
    .join()
    .expect("Runtime creation thread panicked")
}

pub fn create_runner() -> anyhow::Result<TestRunner> {
    let seed = match std::env::var("PROPTEST_SEED") {
        Ok(v) => v
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("PROPTEST_SEED must be a u64: {e}"))?,
        Err(_) => std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs(),
    };
    eprintln!("[pbt_seed] seed={seed} (set PROPTEST_SEED to reproduce)");
    let mut seed_bytes = [0u8; 32];
    seed_bytes[..8].copy_from_slice(&seed.to_le_bytes());
    let rng = TestRng::from_seed(RngAlgorithm::ChaCha, &seed_bytes);
    let config = Config {
        cases: 1,
        failure_persistence: None,
        ..Default::default()
    };
    Ok(TestRunner::new_with_rng(config, rng))
}

pub fn create_initial_ref_state(runner: &mut TestRunner) -> anyhow::Result<VariantRef<Full>> {
    let init_strategy = <VariantRef<Full> as ReferenceStateMachine>::init_state();
    init_strategy
        .new_tree(runner)
        .map_err(|e| anyhow::anyhow!("Failed to generate initial state: {e}"))
        .map(|tree| tree.current())
}

/// Resolve a UI mutation's parameters (parent_id URIs) from a transition.
///
/// Returns `Some((entity, op, resolved_params))` for UI mutations, `None` otherwise.
fn resolve_ui_operation(
    transition: &E2ETransition,
    sut: &E2ESut<Full>,
) -> Option<(String, String, HashMap<String, Value>)> {
    match transition {
        E2ETransition::ApplyMutation(event) if event.source == MutationSource::UI => {
            let (entity, op, params) = event.mutation.to_operation();
            let mut resolved_params = params.clone();
            if let Some(Value::String(pid)) = resolved_params.get("parent_id") {
                let pid_uri = EntityUri::parse(pid).expect("parent_id must be a valid EntityUri");
                let resolved = sut.resolve_uri(&pid_uri);
                resolved_params.insert("parent_id".to_string(), resolved.clone().into());
            }
            Some((entity, op, resolved_params))
        }
        _ => None,
    }
}

/// Run the pre-startup loop: generate and apply transitions until StartApp fires.
///
/// Returns the updated `(ref_state, current_step, actual_steps)`.
fn run_pre_startup_loop(
    runtime: &tokio::runtime::Runtime,
    runner: &mut TestRunner,
    sut: &mut E2ESut<Full>,
    mut ref_state: VariantRef<Full>,
    num_steps: u32,
    label: &str,
) -> anyhow::Result<(VariantRef<Full>, u32, u32)> {
    let mut current_step = 0u32;
    let mut actual_steps = 0u32;
    let mut start_app_done = false;

    while current_step < num_steps && !start_app_done {
        let transition = match generate_transition(runner, &ref_state, current_step)? {
            Some(t) => t,
            None => {
                current_step += 1;
                continue;
            }
        };

        let is_start_app = matches!(&transition, E2ETransition::StartApp { .. });
        ref_state = <VariantRef<Full> as ReferenceStateMachine>::apply(ref_state, &transition);

        runtime.block_on(sut.apply_transition_async(&ref_state, &transition));
        if is_start_app {
            start_app_done = true;
        }
        runtime.block_on(sut.check_invariants_async(&ref_state));
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
        "{label}: exhausted all steps without reaching StartApp"
    );

    Ok((ref_state, current_step, actual_steps))
}

/// Run a single post-startup step with a UiDriver, using block_on for sync execution.
///
/// Returns `true` if a step was executed, `false` if no valid transition was found.
fn run_driver_step(
    runtime: &tokio::runtime::Runtime,
    runner: &mut TestRunner,
    sut: &mut E2ESut<Full>,
    ref_state: &mut VariantRef<Full>,
    current_step: u32,
    num_steps: u32,
    driver: &mut dyn crate::UiDriver,
) -> anyhow::Result<bool> {
    let transition = match generate_transition(runner, ref_state, current_step)? {
        Some(t) => t,
        None => return Ok(false),
    };

    let transition_name = transition.variant_name();
    let ui_op = resolve_ui_operation(&transition, sut);

    *ref_state = <VariantRef<Full> as ReferenceStateMachine>::apply(ref_state.clone(), &transition);

    // Reset OTel span collector so we get per-transition metrics.
    #[cfg(feature = "otel-testing")]
    {
        sut.span_collector.reset();
        sut.last_transition_start = Some(std::time::Instant::now());
        sut.last_transition = transition.clone();
        let rss_now = crate::test_tracing::current_rss_bytes();
        sut.rss_before = rss_now;
        if sut.rss_baseline == 0 {
            sut.rss_baseline = rss_now;
        }
    }

    crate::debug_pause::pause_before_step(current_step + 1, transition_name);

    let highlight_id: Option<String> = ui_op
        .as_ref()
        .and_then(|(_, _, p)| p.get("id"))
        .and_then(|v| v.as_string())
        .map(|s| s.to_string());
    let action_banner = format_action_banner(transition_name, ui_op.as_ref());
    driver.screenshot_overlay(
        transition_name,
        crate::Phase::Pre,
        highlight_id.as_deref(),
        &crate::Overlay::action(action_banner.clone()),
    );

    let post_highlight: Option<String> = if ui_op.is_some() { highlight_id } else { None };

    // Run action + invariants under a single catch_unwind so any panic — from
    // action dispatch (e.g. wait_for_entity_bounds timeout) or from invariant
    // checks — produces a Post screenshot with a red X + the panic message,
    // before the unwind resumes for proptest.
    let outcome = run_step_body_with_post_overlay(
        runtime,
        sut,
        ref_state,
        driver,
        transition_name,
        &action_banner,
        post_highlight.as_deref(),
        ui_op,
        &transition,
    )?;
    let suffix = if outcome.via_ui { " → UI" } else { "" };
    eprintln!(
        "[pbt_step] Step {}/{}: {}{} ✓",
        current_step + 1,
        num_steps,
        transition_name,
        suffix,
    );

    crate::debug_pause::pause_after_step(current_step + 1, transition_name);

    Ok(true)
}

struct StepOutcome {
    via_ui: bool,
}

#[allow(clippy::too_many_arguments)]
fn run_step_body_with_post_overlay(
    runtime: &tokio::runtime::Runtime,
    sut: &mut E2ESut<Full>,
    ref_state: &VariantRef<Full>,
    driver: &mut dyn crate::UiDriver,
    transition_name: &str,
    action_banner: &str,
    highlight: Option<&str>,
    ui_op: Option<(String, String, HashMap<String, Value>)>,
    transition: &E2ETransition,
) -> anyhow::Result<StepOutcome> {
    use futures::FutureExt;
    use std::panic::AssertUnwindSafe;

    let result = runtime.block_on(
        AssertUnwindSafe(async {
            if let Some((entity, op, params)) = ui_op {
                let handled = driver.try_ui_interaction(&entity, &op, &params).await;
                if !handled {
                    // Strict-input mode (PBT_STRICT_INPUT=1) treats this as a
                    // hard failure — every UI op must have a real-input
                    // mapping. New PBT runs should opt in so input-layer
                    // regressions surface here. Default still falls back via
                    // synthetic_dispatch until every op has a gesture mapping.
                    if std::env::var("PBT_STRICT_INPUT").is_ok() {
                        return Err(anyhow::anyhow!(
                            "PBT_STRICT_INPUT: try_ui_interaction returned false for \
                             {entity}.{op} — no real-input mapping for this operation. \
                             Add a gesture path to the UiDriver impl, or unset \
                             PBT_STRICT_INPUT to fall back to synthetic_dispatch."
                        ));
                    }
                    eprintln!(
                        "[pbt_step_confirm] try_ui_interaction returned false for \
                         {entity}.{op} — falling back to synthetic_dispatch \
                         (set PBT_STRICT_INPUT=1 to fail loud instead)"
                    );
                    let drv = sut.driver.as_ref().expect("UserDriver not installed");
                    drv.synthetic_dispatch(&entity, &op, params.clone()).await?;
                }
                driver.settle().await;
                let expected_count = ref_state
                    .block_state
                    .blocks
                    .values()
                    .filter(|b| !b.is_page())
                    .count();
                let timeout = std::time::Duration::from_millis(10000);
                let rows = sut.wait_for_block_count(expected_count, timeout).await;
                if rows.len() != expected_count {
                    eprintln!(
                        "[pbt_step_confirm] WARNING: expected {} blocks, got {}",
                        expected_count,
                        rows.len()
                    );
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                sut.check_invariants_async(ref_state).await;
                Ok(StepOutcome { via_ui: true })
            } else {
                sut.apply_transition_async(ref_state, transition).await;
                sut.check_invariants_async(ref_state).await;
                Ok(StepOutcome { via_ui: false })
            }
        })
        .catch_unwind(),
    );

    match result {
        Ok(Ok(outcome)) => {
            driver.screenshot_overlay(
                transition_name,
                crate::Phase::Post,
                highlight,
                &crate::Overlay::pass(action_banner),
            );
            Ok(outcome)
        }
        Ok(Err(err)) => {
            // anyhow::Error from synthetic_dispatch — surface as Fail overlay
            // so the screenshot shows what went wrong, then propagate.
            driver.screenshot_overlay(
                transition_name,
                crate::Phase::Post,
                highlight,
                &crate::Overlay::fail(action_banner, format!("{err:?}")),
            );
            Err(err)
        }
        Err(payload) => {
            let msg = panic_payload_message(&payload);
            driver.screenshot_overlay(
                transition_name,
                crate::Phase::Post,
                highlight,
                &crate::Overlay::fail(action_banner, msg),
            );
            std::panic::resume_unwind(payload);
        }
    }
}

/// Build a human-readable banner string for the action overlay. Includes the
/// transition variant + key params (entity id) when known.
fn format_action_banner(
    transition_name: &str,
    ui_op: Option<&(String, String, HashMap<String, Value>)>,
) -> String {
    match ui_op {
        Some((entity, op, params)) => {
            let id = params
                .get("id")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string());
            match id {
                Some(id) => format!("{transition_name}  •  {entity}.{op}({id})"),
                None => format!("{transition_name}  •  {entity}.{op}()"),
            }
        }
        None => transition_name.to_string(),
    }
}

/// Run `check_invariants_async` and capture a `Post` screenshot with a
/// pass/fail overlay. On panic, captures `Fail { assertion: <panic msg> }`
/// then resumes the unwind so proptest still sees the failure.
fn run_invariants_with_post_overlay(
    runtime: &tokio::runtime::Runtime,
    sut: &E2ESut<Full>,
    ref_state: &VariantRef<Full>,
    driver: &mut dyn crate::UiDriver,
    transition_name: &str,
    action_banner: &str,
    highlight: Option<&str>,
) {
    use futures::FutureExt;
    use std::panic::AssertUnwindSafe;

    let result =
        runtime.block_on(AssertUnwindSafe(sut.check_invariants_async(ref_state)).catch_unwind());

    match result {
        Ok(()) => {
            driver.screenshot_overlay(
                transition_name,
                crate::Phase::Post,
                highlight,
                &crate::Overlay::pass(action_banner),
            );
        }
        Err(payload) => {
            let msg = panic_payload_message(&payload);
            driver.screenshot_overlay(
                transition_name,
                crate::Phase::Post,
                highlight,
                &crate::Overlay::fail(action_banner, msg),
            );
            std::panic::resume_unwind(payload);
        }
    }
}

fn panic_payload_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "<panic with non-string payload>".to_string()
}

/// Post-startup driver loop: step through remaining transitions with a UiDriver.
///
/// Returns the final `(actual_steps, current_step)`.
fn run_post_startup_driver_loop(
    runtime: &tokio::runtime::Runtime,
    runner: &mut TestRunner,
    sut: &mut E2ESut<Full>,
    ref_state: &mut VariantRef<Full>,
    mut current_step: u32,
    mut actual_steps: u32,
    num_steps: u32,
    driver: &mut dyn crate::UiDriver,
) -> anyhow::Result<(u32, u32)> {
    while current_step < num_steps {
        let stepped = run_driver_step(
            runtime,
            runner,
            sut,
            ref_state,
            current_step,
            num_steps,
            driver,
        )?;
        if stepped {
            actual_steps += 1;
        }
        current_step += 1;
    }
    Ok((actual_steps, current_step))
}

/// Tear down the SUT on a non-async thread.
fn teardown_sut(sut: E2ESut<Full>) {
    std::thread::spawn(move || drop(sut))
        .join()
        .expect("PBT teardown thread panicked");
}

// ──── Phased state machine ────

/// Persistent state across pbt_setup/pbt_step/pbt_teardown calls.
pub struct PbtPhaseState {
    pub sut: E2ESut<Full>,
    pub ref_state: VariantRef<Full>,
    pub runner: TestRunner,
    pub num_steps: u32,
    pub current_step: u32,
    pub actual_steps: u32,
}

// SAFETY: PbtPhaseState contains TestRunner which holds non-Send strategy internals,
// but we only access it from a single logical thread (callers serialize access).
// The Mutex is only used for interior mutability, not cross-thread sharing.
unsafe impl Send for PbtPhaseState {}

static PBT_PHASE_STATE: Mutex<Option<PbtPhaseState>> = Mutex::new(None);

/// Take the phase state out of the mutex (for use across await points).
fn take_phase_state() -> anyhow::Result<PbtPhaseState> {
    PBT_PHASE_STATE
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| anyhow::anyhow!("PBT phase state not initialized — call pbt_setup first"))
}

/// Put the phase state back into the mutex.
fn restore_phase_state(state: PbtPhaseState) {
    *PBT_PHASE_STATE.lock().unwrap() = Some(state);
}

/// Store phase state from external setup (e.g. Flutter's custom pbt_setup).
pub fn store_phase_state(state: PbtPhaseState) {
    *PBT_PHASE_STATE.lock().unwrap() = Some(state);
}

/// Set up the PBT state machine (headless mode — no Flutter).
///
/// Runs all pre-startup transitions + StartApp, installs DirectUserDriver.
/// Returns a summary string. The state is stored in `PBT_PHASE_STATE`.
pub async fn pbt_setup(num_steps: u32) -> anyhow::Result<String> {
    let runtime = create_runtime();
    pbt_setup_with_runtime(num_steps, runtime).await
}

/// Like `pbt_setup` but uses the provided runtime.
async fn pbt_setup_with_runtime(
    num_steps: u32,
    runtime: Arc<tokio::runtime::Runtime>,
) -> anyhow::Result<String> {
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

    // F_direct: install ReactiveEngineDriver so subsequent mutations go
    // through the same dispatch path GPUI uses. Falls back to
    // DirectUserDriver only when ReactiveEngine isn't available (legacy
    // tests that don't wire the reactive runtime).
    if let Some(reactive) = sut.ctx.reactive_engine.as_ref() {
        sut.driver = Some(Box::new(crate::ReactiveEngineDriver::new(reactive.clone())));
    } else {
        sut.driver = Some(Box::new(DirectUserDriver::new(sut.ctx.engine().clone())));
    }

    let summary = format!("setup complete: {actual_steps} pre-startup steps");

    *PBT_PHASE_STATE.lock().unwrap() = Some(PbtPhaseState {
        sut,
        ref_state,
        runner,
        num_steps,
        current_step,
        actual_steps,
    });

    Ok(summary)
}

/// Execute one PBT step.
///
/// For UI mutations: updates reference model, returns operation info.
/// For other transitions: applies normally, returns `ui_operation = None`.
pub async fn pbt_step() -> anyhow::Result<PbtStepResult> {
    let mut state = take_phase_state()?;
    let result = pbt_step_inner(&mut state).await;
    restore_phase_state(state);
    result
}

async fn pbt_step_inner(state: &mut PbtPhaseState) -> anyhow::Result<PbtStepResult> {
    if state.current_step >= state.num_steps {
        return Ok(PbtStepResult {
            done: true,
            transition_name: "done",
            ui_operation: None,
        });
    }

    let mut transition = None;
    while state.current_step < state.num_steps {
        match generate_transition(&mut state.runner, &state.ref_state, state.current_step)? {
            Some(t) => {
                transition = Some(t);
                break;
            }
            None => {
                state.current_step += 1;
            }
        }
    }

    let transition = match transition {
        Some(t) => t,
        None => {
            return Ok(PbtStepResult {
                done: true,
                transition_name: "exhausted",
                ui_operation: None,
            });
        }
    };

    let transition_name = transition.variant_name();

    let ui_op =
        resolve_ui_operation(&transition, &state.sut).map(|(entity, op, resolved_params)| {
            let params_json =
                serde_json::to_string(&resolved_params).expect("params must serialize");
            PbtUiOperation {
                entity,
                op,
                params_json,
                params: resolved_params,
            }
        });

    // Always update reference model
    state.ref_state =
        <VariantRef<Full> as ReferenceStateMachine>::apply(state.ref_state.clone(), &transition);

    if ui_op.is_some() {
        state.current_step += 1;

        eprintln!(
            "[pbt_step] Step {}/{}: {} → UI operation",
            state.current_step, state.num_steps, transition_name,
        );

        Ok(PbtStepResult {
            done: false,
            transition_name,
            ui_operation: ui_op,
        })
    } else {
        state
            .sut
            .apply_transition_async(&state.ref_state, &transition)
            .await;
        state.sut.check_invariants_async(&state.ref_state).await;
        state.actual_steps += 1;
        state.current_step += 1;

        eprintln!(
            "[pbt_step] Step {}/{}: {} ✓",
            state.current_step, state.num_steps, transition_name,
        );

        Ok(PbtStepResult {
            done: false,
            transition_name,
            ui_operation: None,
        })
    }
}

/// Confirm a UI operation has been applied.
///
/// Waits for DB to settle, then runs invariant checks.
pub async fn pbt_step_confirm() -> anyhow::Result<()> {
    let mut state = take_phase_state()?;

    let expected_count = state
        .ref_state
        .block_state
        .blocks
        .values()
        .filter(|b| !b.is_page())
        .count();
    let timeout = std::time::Duration::from_millis(10000);
    let rows = state
        .sut
        .wait_for_block_count(expected_count, timeout)
        .await;
    if rows.len() != expected_count {
        eprintln!(
            "[pbt_step_confirm] WARNING: expected {} blocks, got {} (continuing anyway)",
            expected_count,
            rows.len()
        );
    }

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    state.sut.check_invariants_async(&state.ref_state).await;
    state.actual_steps += 1;

    eprintln!("[pbt_step_confirm] Invariants passed ✓");

    restore_phase_state(state);
    Ok(())
}

/// Tear down the PBT state machine. Returns result summary.
pub async fn pbt_teardown() -> anyhow::Result<String> {
    let state = PBT_PHASE_STATE
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| anyhow::anyhow!("pbt_teardown called before pbt_setup"))?;

    let summary = format!(
        "passed: {}/{} PBT transitions",
        state.actual_steps, state.num_steps
    );

    std::thread::spawn(move || {
        drop(state);
    })
    .join()
    .expect("PBT teardown thread panicked");

    Ok(summary)
}

/// Run the full phased PBT cycle with an optional UiDriver.
///
/// This is the main entry point for headless (FFI-only) cross-frontend testing.
/// When `execute_op` is None, UI operations fall back to direct FFI execution.
pub async fn run_phased_pbt(
    num_steps: u32,
    execute_op: Option<
        &dyn Fn(&PbtUiOperation) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + '_>>,
    >,
) -> anyhow::Result<String> {
    let setup_summary = pbt_setup(num_steps).await?;
    eprintln!("[run_phased_pbt] {setup_summary}");

    loop {
        let step_result = pbt_step().await?;
        if step_result.done {
            break;
        }

        if let Some(ui_op) = &step_result.ui_operation {
            let handled = match execute_op {
                Some(f) => f(ui_op).await,
                None => false,
            };

            if !handled {
                // FFI fallback: execute directly via the SUT's mutation driver
                pbt_execute_operation(&ui_op.entity, &ui_op.op, &ui_op.params).await?;
            }

            pbt_step_confirm().await?;
        }
    }

    pbt_teardown().await
}

/// Run the phased PBT with a `UiDriver` that attempts UI interactions.
///
/// Shared helper used by per-frontend UI PBT tests. The driver's
/// `try_ui_interaction` is called for each UI operation; if it returns
/// `false`, the operation falls back to FFI execution.
pub async fn run_pbt_with_driver(
    num_steps: u32,
    driver: &mut dyn crate::UiDriver,
) -> anyhow::Result<String> {
    let setup_summary = pbt_setup(num_steps).await?;
    eprintln!("[run_pbt_with_driver] {setup_summary}");

    loop {
        let step_result = pbt_step().await?;
        if step_result.done {
            break;
        }

        if let Some(ui_op) = &step_result.ui_operation {
            let handled = driver
                .try_ui_interaction(&ui_op.entity, &ui_op.op, &ui_op.params)
                .await;

            if !handled {
                if std::env::var("PBT_STRICT_INPUT").is_ok() {
                    return Err(anyhow::anyhow!(
                        "PBT_STRICT_INPUT: try_ui_interaction returned false for \
                         {entity}.{op} — no real-input mapping for this operation.",
                        entity = ui_op.entity,
                        op = ui_op.op
                    ));
                }
                pbt_execute_operation(&ui_op.entity, &ui_op.op, &ui_op.params).await?;
            }

            driver.settle().await;
            pbt_step_confirm().await?;
        }
    }

    pbt_teardown().await
}

/// Run the phased PBT synchronously with a `UiDriver`.
///
/// Same runtime-safe pattern as `run_phased_pbt_sync` but routes UI mutations
/// through the driver before falling back to FFI.
///
/// If the driver is a `GeometryDriver` with screenshots enabled, a screenshot
/// is captured after every step (with the interacted element highlighted for
/// UI mutations).
pub fn run_pbt_with_driver_sync(
    num_steps: u32,
    driver: &mut dyn crate::UiDriver,
) -> anyhow::Result<String> {
    run_pbt_with_driver_sync_callback(num_steps, driver, |_| None)
}

/// Like `run_pbt_with_driver_sync`, but calls `on_ready` after StartApp completes.
///
/// The callback receives a `PbtReadyContext` with the BackendEngine, FrontendSession,
/// ReactiveEngine, and runtime handle — everything needed to launch a frontend window
/// sharing the PBT's state (same DB, same DI singletons).
pub fn run_pbt_with_driver_sync_callback(
    num_steps: u32,
    driver: &mut dyn crate::UiDriver,
    on_ready: impl FnOnce(&PbtReadyContext) -> Option<PbtReadyResult>,
) -> anyhow::Result<String> {
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("Failed to create PBT runtime"));

    let mut runner = create_runner()?;
    let ref_state = create_initial_ref_state(&mut runner)?;
    let mut sut = E2ESut::<Full>::new(runtime.clone())?;

    let (mut ref_state, current_step, mut actual_steps) = run_pre_startup_loop(
        &runtime,
        &mut runner,
        &mut sut,
        ref_state,
        num_steps,
        "run_pbt_with_driver_sync_callback",
    )?;

    let ctx = PbtReadyContext {
        engine: sut.ctx.engine().clone(),
        session: sut.ctx.session_arc(),
        reactive_engine: sut
            .ctx
            .reactive_engine
            .clone()
            .expect("ReactiveEngine not initialized after StartApp"),
        runtime_handle: runtime.handle().clone(),
    };
    let ready_result = on_ready(&ctx);
    let (custom_driver, frontend_engine, frontend_geometry, frontend_visual_state) =
        match ready_result {
            Some(r) => (
                r.driver,
                r.frontend_engine,
                r.frontend_geometry,
                r.frontend_visual_state,
            ),
            None => (None, None, None, None),
        };

    // F_direct: prefer ReactiveEngineDriver over DirectUserDriver unless the
    // caller supplied a custom driver (e.g. GpuiUserDriver).
    if custom_driver.is_some() {
        sut.driver = custom_driver;
    } else if let Some(reactive) = sut.ctx.reactive_engine.as_ref() {
        sut.driver = Some(Box::new(crate::ReactiveEngineDriver::new(reactive.clone())));
    } else {
        sut.driver = Some(Box::new(DirectUserDriver::new(sut.ctx.engine().clone())));
    }
    sut.frontend_engine = frontend_engine;
    sut.frontend_geometry = frontend_geometry;
    sut.frontend_visual_state = frontend_visual_state;

    eprintln!(
        "[run_pbt_with_driver_sync_callback] setup complete: {actual_steps} pre-startup steps"
    );

    (actual_steps, _) = run_post_startup_driver_loop(
        &runtime,
        &mut runner,
        &mut sut,
        &mut ref_state,
        current_step,
        actual_steps,
        num_steps,
        driver,
    )?;

    let summary = format!("passed: {actual_steps}/{num_steps} PBT transitions");
    teardown_sut(sut);

    Ok(summary)
}

/// Run the full phased PBT synchronously.
///
/// Uses a single runtime and calls `block_on` per-step (like proptest does).
/// All proptest strategy generation happens OUTSIDE `block_on` to prevent
/// `ReferenceState`'s internal `Arc<Runtime>` from being dropped in an async context.
pub fn run_phased_pbt_sync(num_steps: u32) -> anyhow::Result<String> {
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("Failed to create PBT runtime"));

    let mut runner = create_runner()?;
    let ref_state = create_initial_ref_state(&mut runner)?;
    let mut sut = E2ESut::<Full>::new(runtime.clone())?;

    let (mut ref_state, mut current_step, mut actual_steps) = run_pre_startup_loop(
        &runtime,
        &mut runner,
        &mut sut,
        ref_state,
        num_steps,
        "run_phased_pbt_sync",
    )?;

    // F_direct: prefer ReactiveEngineDriver so post-startup transitions use
    // the reactive dispatch pipeline. Falls back when no ReactiveEngine.
    if let Some(reactive) = sut.ctx.reactive_engine.as_ref() {
        sut.driver = Some(Box::new(crate::ReactiveEngineDriver::new(reactive.clone())));
    } else {
        sut.driver = Some(Box::new(DirectUserDriver::new(sut.ctx.engine().clone())));
    }

    eprintln!("[run_phased_pbt_sync] setup complete: {actual_steps} pre-startup steps");

    // Post-startup step loop (no driver, just apply transitions directly)
    while current_step < num_steps {
        let transition = match generate_transition(&mut runner, &ref_state, current_step)? {
            Some(t) => t,
            None => {
                current_step += 1;
                continue;
            }
        };

        ref_state =
            <VariantRef<Full> as ReferenceStateMachine>::apply(ref_state.clone(), &transition);

        let transition_label = format!("{:?}", std::mem::discriminant(&transition));
        crate::debug_pause::pause_before_step(current_step + 1, &transition_label);

        runtime.block_on(sut.apply_transition_async(&ref_state, &transition));
        runtime.block_on(sut.check_invariants_async(&ref_state));
        actual_steps += 1;
        current_step += 1;
        eprintln!(
            "[pbt_step] Step {}/{}: {} ✓",
            current_step, num_steps, transition_label,
        );

        crate::debug_pause::pause_after_step(current_step, &transition_label);
    }

    let summary = format!("passed: {actual_steps}/{num_steps} PBT transitions");
    teardown_sut(sut);

    Ok(summary)
}

/// Execute a UI operation directly via the SUT's mutation driver (FFI fallback).
///
/// TODO(simulate-real-input): this entire function bypasses the user-input
/// layer. Replace `synthetic_dispatch` with a real chord/click/type pipeline
/// once the Flutter side wraps `send_key_chord` / `click_entity` /
/// `type_text`.
///
/// SYNTHETIC: this is the Dart/Flutter FFI entry point that delegates to
/// `synthetic_dispatch` because the Dart side doesn't yet wrap `send_key_chord`
/// / `click_entity` / `type_text`. When Flutter becomes a first-tier frontend
/// again (see plan `deep-humming-crane.md`), this function should be expanded
/// to route through the user-verb API instead.
pub async fn pbt_execute_operation(
    entity: &str,
    op: &str,
    params: &HashMap<String, Value>,
) -> anyhow::Result<()> {
    let state = take_phase_state()?;

    let driver = state
        .sut
        .driver
        .as_ref()
        .expect("UserDriver not installed — call pbt_setup first");
    driver
        .synthetic_dispatch(entity, op, params.clone())
        .await?;

    restore_phase_state(state);
    Ok(())
}
