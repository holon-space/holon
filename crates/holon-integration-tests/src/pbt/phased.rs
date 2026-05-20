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

use super::E2ETransition;
use super::reference_state::ReferenceState;
use super::types::MutationSource;
use super::{E2ESut, ReferenceMachine};
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
    /// DI-resolved `DebugServices`, populated by
    /// `holon_mcp::di::DebugServicesPopulatorModule` registered in the
    /// test environment's injector. Threaded into the embedded MCP so
    /// inspection tools (`inspect_loro_blocks`, `diff_loro_sql`, etc.)
    /// work during PBT pauses.
    pub debug_services: Arc<holon_mcp::server::DebugServices>,
}

/// Result returned by the `on_ready` callback.
pub struct PbtReadyResult {
    /// Custom mutation driver (None = use DirectUserDriver).
    pub driver: Option<Arc<dyn crate::UserDriver>>,
    /// Optional frontend ReactiveEngine for inv-frontend-engine assertions.
    /// When set, each transition checks the frontend's ViewModel for errors.
    pub frontend_engine: Option<Arc<holon_frontend::reactive::ReactiveEngine>>,
    /// Optional geometry provider for inv-frontend-engine bounds assertions.
    /// When set, checks that GPUI actually laid out the expected elements.
    pub frontend_geometry: Option<Box<dyn holon_frontend::geometry::GeometryProvider>>,
    /// Optional shared screenshot analysis state for inv-frontend-engine empty-UI detection.
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
    ref_state: &ReferenceState,
    step: u32,
) -> anyhow::Result<Option<E2ETransition>> {
    let strategy = <ReferenceMachine as ReferenceStateMachine>::transitions(ref_state);
    let transition = strategy
        .new_tree(runner)
        .map_err(|e| anyhow::anyhow!("Failed to generate transition at step {step}: {e}"))?
        .current();

    if !<ReferenceMachine as ReferenceStateMachine>::preconditions(ref_state, &transition) {
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

pub fn create_initial_ref_state(runner: &mut TestRunner) -> anyhow::Result<ReferenceState> {
    let init_strategy = <ReferenceMachine as ReferenceStateMachine>::init_state();
    init_strategy
        .new_tree(runner)
        .map_err(|e| anyhow::anyhow!("Failed to generate initial state: {e}"))
        .map(|tree| tree.current())
}

/// Resolve a UI mutation's parameters (parent_id URIs) from a transition.
///
/// Returns `Some((entity, op, resolved_params))` for UI mutations, `None` otherwise.
pub(crate) fn resolve_ui_operation(
    transition: &E2ETransition,
    sut: &E2ESut,
) -> Option<(String, String, HashMap<String, Value>)> {
    match transition {
        E2ETransition::ApplyMutation(am) if am.event.source == MutationSource::UI => {
            let event = &am.event;
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
    sut: &mut E2ESut,
    mut ref_state: ReferenceState,
    num_steps: u32,
    label: &str,
) -> anyhow::Result<(ReferenceState, u32, u32)> {
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

        // Record before apply so a panicking step's own transition is in the
        // capture. No-op unless the caller armed capture via `reset_capture`
        // (only the GPUI entry does — `run_pbt_with_driver_sync_callback`).
        crate::pbt::slice::record_transition(&transition);

        let is_start_app = matches!(&transition, E2ETransition::StartApp(_));
        ref_state = <ReferenceMachine as ReferenceStateMachine>::apply(ref_state, &transition);

        runtime.block_on(sut.apply_transition_async(&ref_state, &transition));
        if is_start_app {
            start_app_done = true;
        }
        runtime.block_on(sut.run_invariant_registry(&ref_state));
        actual_steps += 1;
        current_step += 1;
        eprintln!(
            "[pbt_setup] Step {}/{}: {} ✓",
            current_step,
            num_steps,
            transition.variant_name()
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
    sut: &mut E2ESut,
    ref_state: &mut ReferenceState,
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

    // Record before apply so a panicking step's own transition is captured.
    // No-op unless capture was armed by the GPUI entry's `reset_capture`.
    crate::pbt::slice::record_transition(&transition);

    *ref_state = <ReferenceMachine as ReferenceStateMachine>::apply(ref_state.clone(), &transition);

    // Reset per-transition metrics so budgets are scoped per transition.
    // No-op without `otel-testing`.
    sut.last_transition = transition.clone();
    sut.metrics.on_transition_start();

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

    // Fault injection for capture/bisection tooling: panic *after* step N's
    // transition is applied + recorded, so the GPUI capture-on-panic path writes
    // a `tests/.captures/*.json` with a known prefix on demand. Used to exercise
    // the capture→headless-bisect pipeline without hunting for a flaky failing
    // seed (ADR 0009 step 3/4). No effect unless `HOLON_PBT_FORCE_FAIL_AT_STEP`
    // is set to this 1-based step number.
    if let Ok(n) = std::env::var("HOLON_PBT_FORCE_FAIL_AT_STEP") {
        if n.parse::<u32>().ok() == Some(current_step + 1) {
            panic!(
                "HOLON_PBT_FORCE_FAIL_AT_STEP={n}: forced failure after step {} ({transition_name})",
                current_step + 1
            );
        }
    }

    Ok(true)
}

struct StepOutcome {
    via_ui: bool,
}

#[allow(clippy::too_many_arguments)]
fn run_step_body_with_post_overlay(
    runtime: &tokio::runtime::Runtime,
    sut: &mut E2ESut,
    ref_state: &ReferenceState,
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
                    .domain
                    .block_state
                    .blocks
                    .values()
                    .filter(|b| !b.is_page())
                    .count();
                let expected_ids = sut.expected_block_ids(ref_state);
                let timeout = std::time::Duration::from_millis(10000);
                let rows = sut.wait_for_blocks_synced(&expected_ids, timeout).await;
                if rows.len() != expected_count {
                    eprintln!(
                        "[pbt_step_confirm] WARNING: expected {} blocks, got {}",
                        expected_count,
                        rows.len()
                    );
                }
                // No settle sleep: `wait_for_blocks_synced` above is the
                // data barrier and every invariant body polls internally
                // (retry_until_ok), so a fixed pause only added wall time.
                sut.run_invariant_registry(ref_state).await;
                Ok(StepOutcome { via_ui: true })
            } else {
                sut.apply_transition_async(ref_state, transition).await;
                sut.run_invariant_registry(ref_state).await;
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
    sut: &mut E2ESut,
    ref_state: &mut ReferenceState,
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
fn teardown_sut(sut: E2ESut) {
    std::thread::spawn(move || drop(sut))
        .join()
        .expect("PBT teardown thread panicked");
}

// ──── Phased state machine ────

/// Persistent state across pbt_setup/pbt_step/pbt_teardown calls.
pub struct PbtPhaseState {
    pub sut: E2ESut,
    pub ref_state: ReferenceState,
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
    crate::debug_pause::install_panic_pause_hook();
    let runtime = create_runtime();
    pbt_setup_with_runtime(num_steps, runtime).await
}

/// Like `pbt_setup` but uses the provided runtime.
async fn pbt_setup_with_runtime(
    num_steps: u32,
    runtime: Arc<tokio::runtime::Runtime>,
) -> anyhow::Result<String> {
    let mut sut = E2ESut::new(runtime)?;
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

        let is_start_app = matches!(&transition, E2ETransition::StartApp(_));

        ref_state = <ReferenceMachine as ReferenceStateMachine>::apply(ref_state, &transition);
        sut.apply_transition_async(&ref_state, &transition).await;

        if is_start_app {
            start_app_done = true;
        }

        sut.run_invariant_registry(&ref_state).await;
        actual_steps += 1;
        current_step += 1;

        eprintln!(
            "[pbt_setup] Step {}/{}: {} ✓",
            current_step,
            num_steps,
            transition.variant_name()
        );
    }

    assert!(
        start_app_done,
        "pbt_setup exhausted all steps without reaching StartApp"
    );

    // Construct the medium-aware driver once, then install the same
    // `Arc<dyn UserDriver>` into both `sut.driver` (so transitions
    // dispatch through it) and `live_driver()` (so generators read
    // observation verbs from it). The two views must agree on which
    // medium answers — there is no fallback inside generators.
    let driver: Arc<dyn crate::UserDriver> =
        if let Some(reactive) = sut.ctx.reactive_engine.as_ref() {
            Arc::new(crate::ReactiveEngineDriver::new(reactive.clone()))
        } else {
            Arc::new(DirectUserDriver::new(sut.ctx.engine().clone()))
        };
    sut.driver = Some(driver);

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
        <ReferenceMachine as ReferenceStateMachine>::apply(state.ref_state.clone(), &transition);

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
        state.sut.run_invariant_registry(&state.ref_state).await;
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
        .domain
        .block_state
        .blocks
        .values()
        .filter(|b| !b.is_page())
        .count();
    let expected_ids = state.sut.expected_block_ids(&state.ref_state);
    let timeout = std::time::Duration::from_millis(10000);
    let rows = state
        .sut
        .wait_for_blocks_synced(&expected_ids, timeout)
        .await;
    if rows.len() != expected_count {
        eprintln!(
            "[pbt_step_confirm] WARNING: expected {} blocks, got {} (continuing anyway)",
            expected_count,
            rows.len()
        );
    }

    // No settle sleep: `wait_for_blocks_synced` above is the data barrier and
    // every invariant body polls internally (retry_until_ok).
    state.sut.run_invariant_registry(&state.ref_state).await;
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

    // Surface why transitions never fired: each weighted_generator or
    // preconditions rejection has been recorded into a per-thread histogram.
    crate::pbt::validation::print_rejection_histogram();

    std::thread::spawn(move || {
        drop(state);
    })
    .join()
    .expect("PBT teardown thread panicked");

    Ok(summary)
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

/// Run the phased PBT synchronously with a `UiDriver`, calling `on_ready`
/// after StartApp completes.
///
/// The callback receives a `PbtReadyContext` with the BackendEngine, FrontendSession,
/// ReactiveEngine, and runtime handle — everything needed to launch a frontend window
/// sharing the PBT's state (same DB, same DI singletons).
///
/// `wiring` selects the SUT configuration: `Wiring::full()` for the
/// Loro-enabled variant, `Wiring::sql_only()` for the no-Loro / Turso-only
/// one (where editor content is persisted ONLY by the on-blur `set_field`).
/// Each variant is its own test target so both run automatically — see
/// `gpui_ui_pbt` / `gpui_ui_pbt_no_loro` and the TUI twins.
pub fn run_pbt_with_driver_sync_callback(
    wiring: holon_pbt_core::Wiring,
    num_steps: u32,
    driver: &mut dyn crate::UiDriver,
    on_ready: impl FnOnce(&PbtReadyContext) -> Option<PbtReadyResult>,
) -> anyhow::Result<String> {
    // Drop guard so the rejection histogram surfaces even when the PBT
    // panics mid-run (e.g. seed=1 currently hits a Turso scheduler bug at
    // step 44). Without this, every gpui PBT panic loses the histogram —
    // and the histogram is the only signal we have for why FocusEditableText
    // / NavigateFocus never fire.
    struct PrintHistogramOnDrop;
    impl Drop for PrintHistogramOnDrop {
        fn drop(&mut self) {
            crate::pbt::validation::print_rejection_histogram();
        }
    }
    let _histogram_guard = PrintHistogramOnDrop;

    // Capture-on-failure (ADR 0009 step 3 net-new #3): the phased GPUI loop has
    // no `declare_pbt_slice!` wrapper, so it never wrote the JSON capture a
    // headless lattice bisection replays. Arm the same thread-local capture the
    // slice wrapper uses and write it on a panicking unwind, so a UI-observed
    // failure becomes a `tests/.captures/<name>.captured.json` the fast headless
    // bisector can localize. Name overridable via `HOLON_PBT_CAPTURE_NAME`
    // (default `gpui_ui_pbt`).
    let capture_name =
        std::env::var("HOLON_PBT_CAPTURE_NAME").unwrap_or_else(|_| "gpui_ui_pbt".to_string());
    crate::pbt::slice::reset_capture("gpui");
    struct CaptureOnPanic(String);
    impl Drop for CaptureOnPanic {
        fn drop(&mut self) {
            if std::thread::panicking() {
                crate::pbt::slice::write_captured_fixture(&self.0);
            }
        }
    }
    let _capture_guard = CaptureOnPanic(capture_name);

    crate::debug_pause::install_panic_pause_hook();

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("Failed to create PBT runtime"));

    // This is the REAL-editor harness — a GPUI/TUI `UserDriver` drives a live
    // `InputState`, not the headless `HeadlessEditorMirror`. Mark it so the
    // atomic-editor transitions accept SqlOnly runs and the reference commits
    // editor content on blur, mirroring prod's `on_blur` → `set_field`
    // (see `ReferenceState::real_editor_enabled`). SAFETY: set before any
    // transition is generated; the env is process-global and a real-editor
    // PBT binary runs a single mode per process.
    unsafe { std::env::set_var("PBT_REAL_EDITOR", "1") };

    crate::pbt::slice::record_capture_wiring(&wiring);
    eprintln!("[pbt_wiring] {wiring:?}");

    let mut runner = create_runner()?;
    let ref_state = super::fresh_reference_state(wiring);
    let mut sut = E2ESut::new(runtime.clone())?;

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
        debug_services: sut
            .ctx
            .debug_services()
            .cloned()
            .expect("DebugServices not populated — start_app() should have populated it"),
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

    // Build the medium-aware driver and install into both `sut.driver`
    // and `live_driver()`. The caller-supplied `custom_driver` wins —
    // that's how `gpui_ui_pbt.rs` injects `GpuiUserDriver` after the
    // window is up.
    let user_driver: Arc<dyn crate::UserDriver> = match custom_driver {
        Some(d) => d,
        None => match sut.ctx.reactive_engine.as_ref() {
            Some(reactive) => Arc::new(crate::ReactiveEngineDriver::new(reactive.clone())),
            None => Arc::new(DirectUserDriver::new(sut.ctx.engine().clone())),
        },
    };
    sut.driver = Some(user_driver);
    sut.render
        .install_frontend(frontend_engine, frontend_geometry, frontend_visual_state);

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

/// Replay a fixed sequence of `FixtureStep`s through a real frontend.
///
/// Mirrors [`run_pbt_with_driver_sync_callback`] but drives the *given* steps
/// (from a Gherkin `.feature`) instead of generating random ones, with strict
/// semantics: a failed precondition or assertion is a hard error. `on_ready`
/// fires immediately after the StartApp transition is applied (when the
/// `ReactiveEngine` exists), so the caller can launch a window and inject a
/// real driver (e.g. `GpuiUserDriver`) — exactly as the random GPUI PBT does.
/// The reference `wiring` must match what the sequence was generated under,
/// or wiring-gated invariants mis-fire. Captures record their wiring in
/// `Fixture.environment.wiring` — replayers pass that through here.
pub fn replay_fixture_with_driver_sync_callback(
    wiring: holon_pbt_core::Wiring,
    steps: Vec<crate::pbt::fixtures::FixtureStep>,
    on_ready: impl FnOnce(&PbtReadyContext) -> Option<PbtReadyResult>,
    seen_counter: Option<Arc<std::sync::atomic::AtomicUsize>>,
) -> anyhow::Result<String> {
    struct PrintHistogramOnDrop;
    impl Drop for PrintHistogramOnDrop {
        fn drop(&mut self) {
            crate::pbt::validation::print_rejection_histogram();
        }
    }
    let _histogram_guard = PrintHistogramOnDrop;
    crate::debug_pause::install_panic_pause_hook();

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("Failed to create PBT runtime"));
    let ref_state = super::fresh_reference_state(wiring);
    let sut = E2ESut::new(runtime.clone())?;

    let total = steps.len();
    let runtime_for_hook = runtime.clone();
    let mut on_ready = Some(on_ready);

    // The medium-agnostic replay core (shared with the headless slice runner)
    // drives the steps. This hook is the *only* GPUI-specific glue: right after
    // StartApp it assembles the launch context, lets the caller open a window
    // and return a real `GpuiUserDriver`, and installs it into the SUT. Every
    // post-StartApp transition then dispatches through that driver.
    let sut = crate::pbt::fixtures::replay_steps::<ReferenceMachine, E2ESut>(
        "gpui-replay",
        &steps,
        ref_state,
        sut,
        |sut| {
            let Some(callback) = on_ready.take() else {
                return;
            };
            let ctx = PbtReadyContext {
                engine: sut.ctx.engine().clone(),
                session: sut.ctx.session_arc(),
                reactive_engine: sut
                    .ctx
                    .reactive_engine
                    .clone()
                    .expect("ReactiveEngine not initialized after StartApp"),
                runtime_handle: runtime_for_hook.handle().clone(),
                debug_services: sut
                    .ctx
                    .debug_services()
                    .cloned()
                    .expect("DebugServices not populated by StartApp"),
            };
            if let Some(result) = callback(&ctx) {
                if let Some(driver) = result.driver {
                    sut.driver = Some(driver);
                }
                sut.render.install_frontend(
                    result.frontend_engine,
                    result.frontend_geometry,
                    result.frontend_visual_state,
                );
            }
        },
        seen_counter,
    );

    let summary = format!("replayed {total} fixture steps");
    teardown_sut(sut);
    Ok(summary)
}

/// Run the full phased PBT synchronously.
///
/// Uses a single runtime and calls `block_on` per-step (like proptest does).
/// All proptest strategy generation happens OUTSIDE `block_on` to prevent
/// `ReferenceState`'s internal `Arc<Runtime>` from being dropped in an async context.
pub fn run_phased_pbt_sync(num_steps: u32) -> anyhow::Result<String> {
    crate::debug_pause::install_panic_pause_hook();
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("Failed to create PBT runtime"));

    let mut runner = create_runner()?;
    let ref_state = create_initial_ref_state(&mut runner)?;
    let mut sut = E2ESut::new(runtime.clone())?;

    let (mut ref_state, mut current_step, mut actual_steps) = run_pre_startup_loop(
        &runtime,
        &mut runner,
        &mut sut,
        ref_state,
        num_steps,
        "run_phased_pbt_sync",
    )?;

    // Build the medium-aware driver and install into both `sut.driver`
    // and `live_driver()` so generators read the same source.
    let user_driver: Arc<dyn crate::UserDriver> = match sut.ctx.reactive_engine.as_ref() {
        Some(reactive) => Arc::new(crate::ReactiveEngineDriver::new(reactive.clone())),
        None => Arc::new(DirectUserDriver::new(sut.ctx.engine().clone())),
    };
    sut.driver = Some(user_driver);

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
            <ReferenceMachine as ReferenceStateMachine>::apply(ref_state.clone(), &transition);

        let transition_label = transition.variant_name().to_string();
        crate::debug_pause::pause_before_step(current_step + 1, &transition_label);

        runtime.block_on(sut.apply_transition_async(&ref_state, &transition));
        runtime.block_on(sut.run_invariant_registry(&ref_state));
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
