//! Fixture replay — value-level regression persistence.
//!
//! @pbt kind fixture
//! @pbt gen value-level (not seed-level) corpus: a stored `Vec<T>` survives any
//!   generator/strategy edit, so a regression pinned here stays reproducible
//!   even after the distribution that first found it is retuned — the durable
//!   backstop for every gated-arm coverage hole below. `CaptureEnvironment`
//!   records the wiring + env flags a capture ran under; a replay under a
//!   different HOLON_PBT_* gate silently changes the transition alphabet, so
//!   `mismatch_report` must be consulted before trusting a green replay.
//!
//! Proptest's stock `FileFailurePersistence` only stores RNG seeds; any
//! strategy change invalidates them. For PBT slices that share transitions
//! across multiple consumers (cf. PbtSlicing.md), a *value-level* fixture
//! survives strategy edits and is portable between slices that share the
//! same transition type.
//!
//! ## Shape
//!
//! A fixture is `Vec<T>` for some transition type `T` that implements
//! [`TransitionImpl<Ref, Sut>`] **and** `serde::{Serialize, Deserialize}`.
//! The serde bounds live in the helper functions below — never on the
//! [`TransitionImpl`] trait itself, so non-fixture slices pay nothing.
//!
//! Fixtures are JSON on disk; each slice that opts in keeps a sibling
//! `.fixtures/` directory and replays every `.json` file in it before
//! running the proptest sweep.
//!
//! ## What this does NOT do
//!
//! - **Capture failures from proptest runs.** That requires hooking the
//!   proptest test runner; not implemented yet. For now fixtures are
//!   hand-authored from a known-failing transition sequence.
//! - **Validate that a fixture is still in the strategy's domain.** Proptest's
//!   maintainers cite this as why they reject value persistence; we sidestep it
//!   by treating preconditions as gates inside `replay` and reporting *which*
//!   steps no longer apply, so a stale fixture degrades into a clear diagnostic
//!   instead of a silent pass.

use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::invariant::InvariantResult;

/// Editor-shape env flags that change the PBT transition alphabet and
/// reference semantics but live outside the `ComponentSet` lattice
/// (ADR 0009 §4 follow-up: captures must record them or replay is unfaithful).
///
/// `HOLON_FOLDER_COMPANION_SEED` belongs here for the same reason even though
/// it is not editor-shaped: `wide_e2e_ref_for` consults it (via
/// `folder_companion_enabled`) and INSERTS blocks into the reference state, so
/// a capture and a replay that disagree on it start from different states.
pub const CAPTURE_ENV_FLAGS: &[&str] = &["PBT_MUTABLE_TEXT", "HOLON_FOLDER_COMPANION_SEED"];

/// Execution environment a capture/fixture was recorded under.
#[derive(Debug, Clone, Default, PartialEq, Serialize, serde::Deserialize)]
pub struct CaptureEnvironment {
    /// Wiring manifest of the generating run (`None` for hand-authored or
    /// pre-header fixtures).
    #[serde(default)]
    pub wiring: Option<crate::wiring::Wiring>,
    /// Values of [`CAPTURE_ENV_FLAGS`] at record time (unset flags omitted).
    #[serde(default)]
    pub env_flags: std::collections::BTreeMap<String, String>,
}

impl CaptureEnvironment {
    /// Snapshot the recognized env flags from the current process env.
    pub fn current_env_flags() -> std::collections::BTreeMap<String, String> {
        CAPTURE_ENV_FLAGS
            .iter()
            // ALLOW(filter_map_ok): VarError::NotPresent means "flag unset" —
            // omitting it is the documented contract, not a swallowed error.
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
            .collect()
    }

    /// Human-readable mismatch report vs the current process env (and an
    /// optional replay wiring). `None` when everything matches.
    pub fn mismatch_report(&self, replay_wiring: Option<&crate::wiring::Wiring>) -> Option<String> {
        let mut lines = Vec::new();
        let current = Self::current_env_flags();
        if self.env_flags != current {
            lines.push(format!(
                "env flags differ: capture={:?} current={:?}",
                self.env_flags, current
            ));
        }
        if let (Some(captured), Some(replay)) = (self.wiring.as_ref(), replay_wiring) {
            if captured != replay {
                lines.push(format!(
                    "wiring differs: capture={captured:?} replay={replay:?}"
                ));
            }
        }
        (!lines.is_empty()).then(|| lines.join("\n"))
    }
}

/// A replayable sequence of transitions and the invariants to check
/// after every step. Slice-local — instantiate with the slice's
/// concrete transition + (optional) init-state types.
///
/// `I` defaults to `()` so existing fixtures keep their two-field shape;
/// slices that need to pin init-state details (e.g. proptest's randomly-
/// generated `pre_startup_file_count` / `keyword_set`) instantiate it
/// with a custom init type. The runner's `apply_init` closure is the
/// only place that touches `I`, so each slice decides how to lift the
/// captured init into its concrete ref state.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct Fixture<T, I = ()> {
    /// Human-readable name; appears in failure messages.
    pub name: String,
    /// Optional description of what bug the fixture pins.
    #[serde(default)]
    pub description: String,
    /// The environment the capture was recorded under. A capture replayed
    /// under a different wiring or editor-shape env silently changes meaning
    /// (the transition alphabet and ref semantics differ), so record it and
    /// let replayers compare. `#[serde(default)]` keeps old files loading.
    #[serde(default)]
    pub environment: CaptureEnvironment,
    /// Init-state payload, applied to the freshly-constructed ref state
    /// before the transition sequence runs. `#[serde(default)]` so old
    /// fixture files (no `initial_state` field) deserialize cleanly.
    #[serde(default)]
    pub initial_state: I,
    /// The transition sequence to apply.
    pub transitions: Vec<T>,
}

impl<T, I> Fixture<T, I>
where
    T: Serialize + DeserializeOwned,
    I: Serialize + DeserializeOwned + Default,
{
    /// Read a fixture from `path`. Expects JSON matching `Fixture<T>`.
    pub fn load(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let bytes = std::fs::read(path.as_ref())?;
        serde_json::from_slice(&bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Write the fixture to `path` as pretty JSON.
    pub fn save(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path.as_ref(), bytes)
    }
}

/// One step's outcome in a replay run.
#[derive(Debug)]
pub enum StepOutcome {
    /// Step was applied and all invariants passed.
    Ok,
    /// Step was skipped because preconditions no longer hold under the
    /// current ref state — the fixture has drifted.
    SkippedPreconditions(String),
    /// An invariant failed; this is the regression the fixture pins.
    InvariantFailed {
        invariant_id: &'static str,
        msg: String,
    },
}

/// Replay a fixture's transition sequence and check invariants after each
/// successful step.
///
/// The caller supplies:
/// - `preconditions(t, &ref) -> Result<(), reason>`: gate before apply.
/// - `apply_to_ref(t, &mut ref)`: mirror the wide PBT.
/// - `apply_to_sut(t, &ref, &mut sut)`: async; same.
/// - `check_invariants(&ref, &sut) -> async Result<(), InvariantFailure>`: runs
///   every invariant the slice cares about. The closure form sidesteps the
///   dyn-trait restriction on `async fn` in traits; each slice already has a
///   `check_invariants` method emitted by `declare_pbt_slice!`, so wiring is
///   one line.
///
/// Returns `Vec<StepOutcome>` so the caller decides whether a
/// `SkippedPreconditions` outcome is soft (drift) or hard
/// (regression-now-stale).
pub async fn replay<T, I, Ref, Sut, Apply, Check>(
    fixture: &Fixture<T, I>,
    ref_state: &mut Ref,
    sut: &mut Sut,
    preconditions: impl Fn(&T, &Ref) -> Result<(), String>,
    mut apply_to_ref: impl FnMut(&T, &mut Ref),
    mut apply_to_sut: Apply,
    mut check_invariants: Check,
) -> Vec<StepOutcome>
where
    T: std::fmt::Debug,
    Apply: for<'a> AsyncFnMut(&'a T, &'a Ref, &'a mut Sut) -> (),
    Check: for<'a> AsyncFnMut(&'a Ref, &'a Sut) -> Result<(), InvariantFailure>,
{
    let mut out = Vec::with_capacity(fixture.transitions.len());
    for (i, t) in fixture.transitions.iter().enumerate() {
        if let Err(reason) = preconditions(t, ref_state) {
            out.push(StepOutcome::SkippedPreconditions(format!(
                "step {i} ({t:?}): {reason}"
            )));
            continue;
        }
        apply_to_ref(t, ref_state);
        apply_to_sut(t, &*ref_state, sut).await;
        match check_invariants(&*ref_state, &*sut).await {
            Ok(()) => out.push(StepOutcome::Ok),
            Err(InvariantFailure { invariant_id, msg }) => {
                out.push(StepOutcome::InvariantFailed { invariant_id, msg });
                return out;
            }
        }
    }
    out
}

/// Reported back from the caller's `check_invariants` closure.
#[derive(Debug)]
pub struct InvariantFailure {
    pub invariant_id: &'static str,
    pub msg: String,
}

impl InvariantFailure {
    /// Construct from an [`InvariantResult::Fail`] when the caller is
    /// iterating multiple invariants by hand.
    pub fn from_result(invariant_id: &'static str, r: InvariantResult) -> Result<(), Self> {
        match r {
            InvariantResult::Ok | InvariantResult::Skipped(_) => Ok(()),
            InvariantResult::Fail(msg) => Err(Self { invariant_id, msg }),
        }
    }
}

/// Convenience: panic on the first hard failure (invariant fail).
/// Soft failures (`SkippedPreconditions`) are reported via the
/// returned Vec for the caller to inspect.
pub fn assert_no_invariant_failures(outcomes: &[StepOutcome]) {
    for o in outcomes {
        if let StepOutcome::InvariantFailed { invariant_id, msg } = o {
            panic!("[fixture replay] {invariant_id}: {msg}");
        }
    }
}
