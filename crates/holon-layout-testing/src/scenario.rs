//! `Scenario`, `StepInput`, `run_scenario` — the closure-driven scenario runner.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use holon_frontend::reactive_view_model::ReactiveViewModel;
use proptest::test_runner::TestCaseError;

use crate::blueprint::Blueprint;
use crate::invariants::assert_layout_ok;
use crate::registry::{BlockTreeRegistry, BlockTreeThunk};
use crate::scene_state::SceneState;
use crate::snapshot::BoundsSnapshot;
use crate::ui_interaction::UiInteraction;

// ── Scenario ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct Scenario {
    pub blueprint: Blueprint,
    pub actions: Vec<UiInteraction>,
}

impl fmt::Debug for Scenario {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tree = self.blueprint.shape.materialize();
        writeln!(f, "\nScenario:")?;
        writeln!(f, "  Tree:")?;
        write!(f, "{}", tree.snapshot().pretty_print(2))?;
        writeln!(f, "  Handles: {}", self.blueprint.handles.len())?;
        for h in &self.blueprint.handles {
            writeln!(f, "    {} (modes: {})", h.block_id, h.mode_names.join(", "))?;
        }
        writeln!(f, "  Drawers: {}", self.blueprint.drawers.len())?;
        for d in &self.blueprint.drawers {
            writeln!(f, "    {}", d.block_id)?;
        }
        writeln!(f, "  Actions: {}", self.actions.len())?;
        for (i, a) in self.actions.iter().enumerate() {
            writeln!(f, "    {i}: {a:?}")?;
        }
        Ok(())
    }
}

impl Scenario {
    pub fn materialize(&self) -> Arc<ReactiveViewModel> {
        Arc::new(self.blueprint.shape.materialize())
    }

    /// Block registration data using each handle's `initial_mode`.
    pub fn block_registrations(&self) -> Vec<(String, Vec<(String, BlockTreeThunk)>, usize)> {
        self.blueprint
            .handles
            .iter()
            .map(|h| {
                let modes: Vec<(String, BlockTreeThunk)> = h
                    .mode_names
                    .iter()
                    .cloned()
                    .zip(
                        h.mode_thunks
                            .as_ref()
                            .expect("scenario.block_registrations requires blueprint-sourced handles with mode_thunks")
                            .iter()
                            .cloned(),
                    )
                    .collect();
                (h.block_id.clone(), modes, h.initial_mode)
            })
            .collect()
    }

    /// Block registration data with `overrides` applied to starting modes.
    pub fn block_registrations_with_overrides(
        &self,
        overrides: &HashMap<String, String>,
    ) -> Vec<(String, Vec<(String, BlockTreeThunk)>, usize)> {
        self.blueprint
            .handles
            .iter()
            .map(|h| {
                let active_idx = overrides
                    .get(&h.block_id)
                    .and_then(|m| h.mode_names.iter().position(|n| n == m))
                    .unwrap_or(h.initial_mode);
                let modes: Vec<(String, BlockTreeThunk)> = h
                    .mode_names
                    .iter()
                    .cloned()
                    .zip(
                        h.mode_thunks
                            .as_ref()
                            .expect("scenario.block_registrations requires blueprint-sourced handles with mode_thunks")
                            .iter()
                            .cloned(),
                    )
                    .collect();
                (h.block_id.clone(), modes, active_idx)
            })
            .collect()
    }
}

// ── StepInput ─────────────────────────────────────────────────────────────

/// Input passed to the per-frontend closure on each call to `run_scenario`.
///
/// `Mount` may be called more than once per `run_scenario` invocation (once
/// for the reference render, once for the test render). When the closure
/// sees a second `Mount`, it must drop any open session (closing the window)
/// before opening a new one. Using `Option<GpuiScenarioSession>` and
/// assigning `session = Some(...)` naturally drops the old session.
pub enum StepInput<'a> {
    /// Open (or re-open) the frontend with the given tree and block registrations.
    /// Return a snapshot after settling.
    ///
    /// `state` carries the full UI state the session must pre-apply
    /// before first paint — modes via `state.active_modes`, drawer
    /// open/closed via `state.closed_drawers()`, collapsibles via
    /// `state.collapsed_rows()`. Adding a new dimension is one new
    /// field on `SceneState` plus one new session-side pre-apply; no
    /// new `StepInput` field needed.
    Mount {
        vm: Arc<ReactiveViewModel>,
        /// Pass to `BlockTreeRegistry::register` for each entry.
        blocks: Vec<(String, Vec<(String, BlockTreeThunk)>, usize)>,
        /// Final UI state to pre-apply before first paint. For the test
        /// mount this is `SceneState::from_handles(...)` (defaults
        /// everywhere); for the reference mount it's
        /// `SceneState::replay(..., &actions)`.
        state: SceneState,
    },
    /// Apply the given interaction, settle, and return a new snapshot.
    Apply(&'a UiInteraction),
}

// ── Helpers ───────────────────────────────────────────────────────────────

/// Register all scenario blocks into `registry` with default (index-0) modes.
pub fn register_scenario_blocks(scenario: &Scenario, registry: &BlockTreeRegistry) {
    for (block_id, modes, active_idx) in scenario.block_registrations() {
        registry.register(block_id, modes, active_idx);
    }
}

// ── run_scenario ──────────────────────────────────────────────────────────

/// Run a scenario through the reference + test rendering paths.
///
/// Calls the step closure with `StepInput::Mount` **twice** (once for the
/// reference render with final modes pre-applied, once for the test render
/// with initial modes), then with `StepInput::Apply(action)` for each action.
///
/// Checks layout invariants after the reference render, after the initial
/// test render, and after each action. Finally asserts structural-dump
/// equality between the reference end state and the test end state.
///
/// The step closure is responsible for the full frontend lifecycle:
/// - On `Mount`: drop any prior session, open a new window/backend with the
///   given `vm` and registered `blocks`, settle, and return a snapshot.
/// - On `Apply`: apply the action through the real frontend, settle, snapshot.
pub fn run_scenario<F>(scenario: &Scenario, mut step: F) -> Result<(), TestCaseError>
where
    F: FnMut(StepInput<'_>) -> BoundsSnapshot,
{
    let reference_state = SceneState::replay(
        scenario.blueprint.handles.clone(),
        scenario.blueprint.drawers.clone(),
        scenario.blueprint.collapsibles.clone(),
        &scenario.actions,
    );

    // ── Reference render: final state (all dimensions) pre-applied ──
    let reference_snap = step(StepInput::Mount {
        vm: scenario.materialize(),
        blocks: scenario.block_registrations_with_overrides(&reference_state.active_modes),
        state: reference_state,
    });
    catch_invariant(|| assert_layout_ok(&reference_snap, "proptest.reference"))?;
    let reference_dump = reference_snap.structural_dump();

    // ── Test render: defaults everywhere, replay actions ──
    let test_initial_state = SceneState::from_handles(
        scenario.blueprint.handles.clone(),
        scenario.blueprint.drawers.clone(),
        scenario.blueprint.collapsibles.clone(),
    );
    let initial_snap = step(StepInput::Mount {
        vm: scenario.materialize(),
        blocks: scenario.block_registrations(),
        state: test_initial_state,
    });
    catch_invariant(|| assert_layout_ok(&initial_snap, "proptest.test.initial"))?;

    let mut last_snap = initial_snap;
    for (i, action) in scenario.actions.iter().enumerate() {
        let snap = step(StepInput::Apply(action));
        let label = format!("proptest.test.after_{i}");
        catch_invariant(|| assert_layout_ok(&snap, &label))?;
        last_snap = snap;
    }

    let final_dump = last_snap.structural_dump();
    if final_dump != reference_dump {
        return Err(TestCaseError::fail(format!(
            "end-state equivalence violated\n\
             Applying the action sequence to the initial state produced a \
             different structural dump than building the scenario with the \
             final modes pre-applied.\n\n\
             === Reference (final modes pre-applied) ===\n{reference_dump}\n\
             === Test (initial → actions replayed) ===\n{final_dump}"
        )));
    }

    Ok(())
}

/// Convert a panicking invariant call into `Err(TestCaseError)` so proptest
/// can shrink the input rather than aborting the runner.
fn catch_invariant(f: impl FnOnce() + std::panic::UnwindSafe) -> Result<(), TestCaseError> {
    std::panic::catch_unwind(f).map_err(|e| TestCaseError::fail(panic_to_string(e)))
}

fn panic_to_string(e: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = e.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = e.downcast_ref::<&str>() {
        s.to_string()
    } else {
        "invariant violation (non-string panic payload)".to_string()
    }
}
