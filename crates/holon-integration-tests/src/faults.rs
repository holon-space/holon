//! Substrate-corruption fault injection for the composed keystone family.
//!
//! Two fault "transitions" — [`CorruptTurso`] and [`CorruptLoro`] — damage the
//! derived SQL layer (Turso) or the persisted CRDT store (Loro) in targeted,
//! REAL-failure-class shapes, at two [`CorruptionTiming`]s: while the app is
//! running (`MidRun`) and across a restart (`PreRestart`, i.e. corrupt the
//! persisted artifact between a clean `stop_app` and the next `start_app`).
//!
//! ## Why these live BESIDE the keystone, not woven into `E2ETransition`
//!
//! The one composed keystone (`general_e2e_composed_pbt`) boots PRE-STARTED and
//! has **no true storage reboot** — `SimulateRestart` only touch-writes org
//! files to re-ingest; nothing drops+reopens the Turso handle or rehydrates
//! Loro from disk (the real reboot is the unbuilt "F9" fork). So the
//! `PreRestart` timing is structurally unreachable there, and every corruption
//! shape breaks the keystone's steady invariants BY CONSTRUCTION — it can never
//! be a green member of the sequential alphabet. This mirrors the existing
//! `matview_reboot_duplicate_repro` precedent ("kept: no reboot transition in
//! keystone").
//!
//! Therefore these are exercised through [`crate::test_environment`]
//! (`TestEnvironment`), which DOES support plant-corrupt-then-boot and
//! stop/corrupt/reboot over a real on-disk `test.db` + real-disk Loro snapshot,
//! and are driven by the `#[ignore]`d red-guard rungs in
//! `tests/substrate_corruption_faults.rs`. Each guard asserts the DESIRED
//! outcome from the BootLadder plan and, where today's behavior falls short,
//! carries the OBSERVED failure mode in its `#[ignore]` string. When the
//! BootLadder recovery increments land, the guards flip green; only then are
//! these shapes candidates for weaving into the drawn `E2ETransition` enum
//! (wrap [`TursoCorruption::ALL`] / [`LoroCorruption::ALL`] in a proptest
//! selector).
//!
//! ## The oracle: "no silent wrongness", not "everything works"
//!
//! After a corruption + a drive of the UI-facing read path, [`classify`]
//! records ONE of [`CorruptionOutcome`]. The invariant is that the app must
//! NEVER land in [`CorruptionOutcome::SilentDataLoss`] (present clean/empty or
//! reduced data with no error and no disclosure) — a typed `Error`, a disclosed
//! `ObservedProblem`, or a post-injection `Panic` are all acceptable-today
//! rungs on the fail-loud ladder; silent data loss is the one forbidden floor.
//!
//! Two rules keep the ladder from certifying nothing, both learned the hard way
//! (see `docs/Testing/CorruptionFailureModes-2026-07-18.md`):
//!
//! - The row-count check runs FIRST, so no other rung can mask the floor, and
//!   only FAULT-ATTRIBUTED disclosure lifts an observed loss off it — ambient
//!   `stop_app` noise must never launder a silent loss into `ObservedProblem`.
//! - A panic BEFORE the injection lands is a [`SetupFailure`], never a rung.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use holon_api::QueryLanguage;
use holon_pbt_core::types::LoroCorruptionType;

use crate::test_environment::TestEnvironment;

/// A minimal three-block vault whose blocks carry stable ids, so a boot's
/// `block_raw` population is deterministically countable across a reboot.
pub const CORRUPTION_VAULT_ORG: &str = "\
* Alpha
:PROPERTIES:
:ID: corrupt-a
:END:
* Beta
:PROPERTIES:
:ID: corrupt-b
:END:
* Gamma
:PROPERTIES:
:ID: corrupt-c
:END:
";

/// How the derived Turso SQL layer is damaged. Each shape mirrors a real
/// failure class we have seen in the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TursoCorruption {
    /// `DROP TABLE block_raw` against the live handle — the canonical base
    /// table every matview projects from. Mirrors the class where matview
    /// reconcile used to DROP Turso system tables out from under the readers.
    DropBlockRawTable,
    /// `DROP` a `__turso_internal_dbsp_state_v1_*` table (discovered via
    /// `sqlite_master`) — the serialized DBSP/IVM state. Mirrors the Android
    /// stale-epoch DBSP-state orphan class (state survives a reopen and no
    /// longer matches the base rows).
    DropDbspStateTable,
    /// Truncate the on-disk `test.db` to a few bytes. Only bites on the next
    /// open, so it is a `PreRestart` shape; applied mid-run it is latent.
    TruncateDbFile,
}

impl TursoCorruption {
    pub const ALL: [Self; 3] = [
        Self::DropBlockRawTable,
        Self::DropDbspStateTable,
        Self::TruncateDbFile,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Self::DropBlockRawTable => "drop-block-raw",
            Self::DropDbspStateTable => "drop-dbsp-state",
            Self::TruncateDbFile => "truncate-db-file",
        }
    }
}

/// How the persisted Loro snapshot (`holon_tree.loro`) is damaged. Reuses the
/// existing [`LoroCorruptionType`] byte-shapes plus a whole-file delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoroCorruption {
    /// Overwrite the snapshot with invalid magic bytes
    /// ([`LoroCorruptionType::InvalidHeader`]).
    CorruptSnapshotBytes,
    /// Overwrite the snapshot with a truncated Loro header
    /// ([`LoroCorruptionType::Truncated`]).
    TruncateSnapshot,
    /// Delete the snapshot file entirely.
    DeleteSnapshot,
}

impl LoroCorruption {
    pub const ALL: [Self; 3] = [
        Self::CorruptSnapshotBytes,
        Self::TruncateSnapshot,
        Self::DeleteSnapshot,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Self::CorruptSnapshotBytes => "corrupt-snapshot-bytes",
            Self::TruncateSnapshot => "truncate-snapshot",
            Self::DeleteSnapshot => "delete-snapshot",
        }
    }
}

/// WHEN the corruption is injected relative to the app lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorruptionTiming {
    /// Damage the substrate while the app is running, then drive a read.
    MidRun,
    /// Boot clean, `stop_app`, damage the persisted artifact, then `start_app`
    /// again over the same dir and drive a read (corruption "in combination
    /// with StartApp").
    PreRestart,
}

impl CorruptionTiming {
    pub fn slug(self) -> &'static str {
        match self {
            Self::MidRun => "mid-run",
            Self::PreRestart => "pre-restart",
        }
    }
}

/// The observed process outcome of a corruption scenario — the fail-loud ladder
/// (best → worst last is `SilentDataLoss`, the one forbidden floor).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorruptionOutcome {
    /// A read after corruption returned MORE OR EQUAL canonical rows than
    /// before with no error — the substrate absorbed the fault (or recovered
    /// from a redundant source such as org re-ingest). Detail says which.
    Survived,
    /// Boot or the UI-facing read returned a typed `Err` — fail-loud, the app
    /// did not fake data. Acceptable today.
    TypedError,
    /// An ERROR-level tracing event or a background-thread panic was captured
    /// by the process-global collector (disclosed, not swallowed). Requires the
    /// `otel-testing` feature to observe; acceptable today.
    ObservedProblem,
    /// A panic unwound out of the boot/read on the driver thread. Fail-loud but
    /// unstructured (priority-3). Acceptable-today only until BootLadder Inc 1
    /// converts it to a typed `BootError`.
    Panic,
    /// The substrate REFUSED the injection, so nothing was corrupted and the
    /// scenario proves nothing about recovery. Distinct from [`Self::Survived`]
    /// on purpose: reporting a rejected injection as "survived" would be the
    /// harness's own silent-degradation floor.
    FaultRejected,
    /// FORBIDDEN: the read SUCCEEDED and presented FEWER canonical rows (down
    /// to zero) than before the corruption, with NO error and NO disclosure
    /// — the app silently faked a clean/empty vault over a damaged store.
    SilentDataLoss,
}

impl CorruptionOutcome {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Survived => "Survived",
            Self::TypedError => "TypedError",
            Self::ObservedProblem => "ObservedProblem",
            Self::Panic => "Panic",
            Self::FaultRejected => "FaultRejected",
            Self::SilentDataLoss => "SilentDataLoss",
        }
    }
}

/// The full record of one corruption scenario for the failure-modes ledger.
#[derive(Debug, Clone)]
pub struct ScenarioReport {
    pub outcome: CorruptionOutcome,
    /// Human-readable evidence: pre/post row counts, boot/read Result
    /// summaries, captured problems.
    pub detail: String,
}

impl ScenarioReport {
    fn new(outcome: CorruptionOutcome, detail: impl Into<String>) -> Self {
        Self {
            outcome,
            detail: detail.into(),
        }
    }
}

/// One captured problem, with the emitting module's `target` kept SEPARATE from
/// the message. Attribution reads `message` only: `CapturedProblem`'s `Display`
/// interpolates the target, so matching the rendered form makes every
/// `holon_loro::*` module path answer to the token "loro" no matter what it
/// actually said.
#[derive(Debug, Clone)]
struct Problem {
    /// The `error!` body — the ONLY field attribution may look at.
    message: String,
    /// `[{kind}] {target} ({loc}): {message}`, for the ledger detail line.
    rendered: String,
}

/// Drain the process-global swallowed-problem collector (ERROR logs +
/// background-thread panics). No-op without `otel-testing` (returns empty), so
/// the guards still run — panics on the driver thread and typed `Err`s are the
/// feature-free signals; this only adds background-worker disclosure.
fn captured_problems() -> Vec<Problem> {
    #[cfg(feature = "otel-testing")]
    {
        crate::test_tracing::SpanCollector::global()
            .captured_problems()
            .iter()
            .map(|p| Problem {
                message: p.message.clone(),
                rendered: p.to_string(),
            })
            .collect()
    }
    #[cfg(not(feature = "otel-testing"))]
    {
        Vec::new()
    }
}

fn reset_problems() {
    #[cfg(feature = "otel-testing")]
    {
        crate::test_tracing::SpanCollector::global().reset();
    }
}

/// Count rows in a relation, or `Err` text if the read itself failed (e.g. the
/// table was dropped) — the caller distinguishes a failed read (fail-loud) from
/// an empty result (candidate silent-data-loss).
async fn count_rows(env: &TestEnvironment, relation: &str) -> Result<i64, String> {
    let sql = format!("SELECT COUNT(*) AS c FROM {relation}");
    match env.query(&sql, QueryLanguage::HolonSql).await {
        // A COUNT(*) that came back without an integer `c` is a broken
        // harness contract, not a row count — surfacing it as a sentinel
        // (-1) would make it compare as "fewer rows" and forge the floor.
        Ok(rows) => rows
            .first()
            .and_then(|r| r.get("c"))
            .and_then(|v| v.as_i64())
            .ok_or_else(|| format!("COUNT(*) over {relation} returned no integer `c`: {rows:?}")),
        Err(e) => Err(format!("{e:#}")),
    }
}

/// Wait until the org scan lands the three seeded blocks in `block_raw`.
/// `Err` when the seed never appears — a SETUP failure, which the caller must
/// surface as a hard test failure rather than a corruption-scenario outcome.
async fn wait_for_seed(env: &TestEnvironment) -> Result<(), String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(25);
    loop {
        let rows = env
            .query_sql(
                "SELECT id FROM block_raw WHERE id IN ('block:corrupt-a', 'block:corrupt-b', \
                 'block:corrupt-c')",
            )
            .await;
        if let Ok(rows) = &rows
            && rows.len() >= 3
        {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "org scan never populated all three seed blocks within 25s: {rows:?}"
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Execute one raw SQL statement against the live db handle, bypassing query
/// compilation (DDL/DML). Returns the error text on failure.
async fn raw_sql(env: &TestEnvironment, sql: &str) -> Result<(), String> {
    env.test_ctx()
        .service()
        .execute_raw_sql(sql, HashMap::new())
        .await
        .map(|_| ())
        .map_err(|e| format!("{e:#}"))
}

/// The on-disk `test.db` path this environment opens.
fn turso_db_path(env: &TestEnvironment) -> std::path::PathBuf {
    env.temp_dir.path().join("test.db")
}

/// The on-disk `holon_tree.loro` snapshot path, resolved from the live store's
/// `storage_dir`. `None` when Loro is disabled or not yet started.
async fn loro_snapshot_path(env: &TestEnvironment) -> Option<std::path::PathBuf> {
    let store = env.loro_doc_store()?;
    let dir = store.read().await.storage_dir().to_path_buf();
    Some(dir.join("holon_tree.loro"))
}

/// Force the live Loro store to write its snapshot to disk, so the PreRestart
/// shapes damage REAL persisted data instead of planting a novel file.
/// `wait_for_loro_quiescence` only settles the sync controller; it does not
/// promise a flush, which left the injector fabricating a 4-byte file most
/// runs.
async fn persist_loro_snapshot(env: &TestEnvironment) -> Result<(), String> {
    let store = env
        .loro_doc_store()
        .ok_or_else(|| "Loro is disabled in this environment".to_string())?;
    store
        .read()
        .await
        .save_all()
        .await
        .map_err(|e| format!("save_all failed: {e:#}"))
}

/// Overwrite/delete the Loro snapshot with the shape's bytes.
///
/// `Ok(description)` = real persisted data was damaged. `Err(description)` =
/// there was nothing on disk to damage, which is [`CorruptionOutcome::
/// FaultRejected`]: fabricating a file and calling it an injection would make
/// the guard assert against a fault that never existed.
fn damage_loro_file(
    path: Option<&std::path::Path>,
    shape: LoroCorruption,
) -> Result<String, String> {
    let Some(path) = path else {
        return Err("no Loro snapshot path (Loro disabled?)".to_string());
    };
    if !path.exists() {
        return Err(format!(
            "no persisted snapshot at {} — nothing to corrupt",
            path.display()
        ));
    }
    let before = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    match shape {
        LoroCorruption::DeleteSnapshot => {
            std::fs::remove_file(path).map_err(|e| format!("remove snapshot: {e}"))?;
            Ok(format!("deleted {} ({before} bytes)", path.display()))
        }
        LoroCorruption::CorruptSnapshotBytes | LoroCorruption::TruncateSnapshot => {
            let ty = if matches!(shape, LoroCorruption::CorruptSnapshotBytes) {
                LoroCorruptionType::InvalidHeader
            } else {
                LoroCorruptionType::Truncated
            };
            let bytes: Vec<u8> = match ty {
                LoroCorruptionType::Empty => Vec::new(),
                LoroCorruptionType::Truncated => vec![0x4C, 0x6F, 0x72, 0x6F],
                LoroCorruptionType::InvalidHeader => vec![0xFF, 0xFE, 0x00, 0x01],
            };
            std::fs::write(path, &bytes).map_err(|e| format!("overwrite snapshot: {e}"))?;
            Ok(format!(
                "overwrote {} ({before} bytes) with {:?} ({} bytes)",
                path.display(),
                ty,
                bytes.len()
            ))
        }
    }
}

/// Inject a Turso corruption against the live handle (SQL shapes).
///
/// `Ok(description)` = the damage LANDED; `Err(description)` = the substrate
/// refused it, so the scenario is vacuous and must be reported as
/// [`CorruptionOutcome::FaultRejected`] rather than `Survived`.
/// `TruncateDbFile` is latent here (it only bites the file on the next open,
/// handled in the `PreRestart` path).
async fn inject_turso_live(
    env: &TestEnvironment,
    shape: TursoCorruption,
) -> Result<String, String> {
    match shape {
        TursoCorruption::DropBlockRawTable => {
            // `block_raw` is the FK parent of the derived tables, so a bare
            // DROP is rejected. Disabling FK enforcement for the statement is
            // what makes this shape reproduce the real "base table vanished
            // under the readers" class instead of no-opping.
            let pragma = raw_sql(env, "PRAGMA foreign_keys = OFF").await;
            let r = raw_sql(env, "DROP TABLE IF EXISTS block_raw").await;
            let desc =
                format!("PRAGMA foreign_keys=OFF -> {pragma:?}; DROP TABLE block_raw -> {r:?}");
            match r {
                Ok(()) => Ok(desc),
                Err(_) => Err(desc),
            }
        }
        TursoCorruption::DropDbspStateTable => {
            let listed = env
                .query_sql(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE \
                     '__turso_internal_dbsp_state_v1_%' LIMIT 1",
                )
                .await;
            let target = listed.ok().and_then(|rows| {
                rows.first()
                    .and_then(|r| r.get("name"))
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string())
            });
            match target {
                Some(name) => {
                    let r = raw_sql(env, &format!("DROP TABLE IF EXISTS \"{name}\"")).await;
                    let desc = format!("DROP dbsp-state table {name} -> {r:?}");
                    match r {
                        Ok(()) => Ok(desc),
                        Err(_) => Err(desc),
                    }
                }
                None => Err("no __turso_internal_dbsp_state_v1_* table found".to_string()),
            }
        }
        TursoCorruption::TruncateDbFile => Ok("no-op mid-run (latent file shape)".to_string()),
    }
}

/// The substrate refused the injection: record it as its own outcome so a
/// vacuous run can never be read as evidence that the app survived a fault.
fn rejected_report(layer: &str, shape: &str, timing: &str, refused: &str) -> ScenarioReport {
    ScenarioReport::new(
        CorruptionOutcome::FaultRejected,
        format!("{layer}/{shape}/{timing}: substrate refused the injection: {refused}"),
    )
}

/// A failure in the SETUP phase — before any corruption was injected. Never a
/// scenario outcome: a run that never reached its fault proves nothing, so the
/// caller must turn this into a hard test failure. Round 1 of this port shipped
/// 7/7 guards passing on setup panics laundered through the `Panic` rung; that
/// hole is closed by making the phase boundary explicit and typed.
#[derive(Debug)]
pub struct SetupFailure(pub String);

impl std::fmt::Display for SetupFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SETUP FAILURE (no corruption was injected): {}", self.0)
    }
}

/// Boot a seeded app and take the pre-corruption baseline. Everything here is
/// setup: a failure means the scenario never got to test anything.
async fn setup_seeded_app(
    env: &TestEnvironment,
) -> Result<(Result<i64, String>, Result<i64, String>), SetupFailure> {
    env.write_org_file("vault.org", CORRUPTION_VAULT_ORG)
        .await
        .map_err(|e| SetupFailure(format!("write vault.org: {e:#}")))?;
    env.start_app(true)
        .await
        .map_err(|e| SetupFailure(format!("boot-1 start_app: {e:#}")))?;
    wait_for_seed(env).await.map_err(SetupFailure)?;
    let pre_raw = count_rows(env, "block_raw").await;
    let pre_ui = count_rows(env, "block").await;
    if let Err(e) = &pre_raw {
        return Err(SetupFailure(format!(
            "baseline block_raw count failed: {e}"
        )));
    }
    Ok((pre_raw, pre_ui))
}

/// Run a full Turso-corruption scenario end to end and record its outcome.
///
/// `injected` is set the moment the damage lands, so the caller can tell a
/// post-injection panic (a legitimate [`CorruptionOutcome::Panic`] rung) from a
/// setup panic (never an outcome — a hard failure).
pub async fn run_turso_scenario(
    env: &mut TestEnvironment,
    shape: TursoCorruption,
    timing: CorruptionTiming,
    injected: &AtomicBool,
) -> Result<ScenarioReport, SetupFailure> {
    let (pre_raw, pre_ui) = setup_seeded_app(env).await?;
    reset_problems();

    let (boot_note, effect) = match timing {
        CorruptionTiming::MidRun => match inject_turso_live(env, shape).await {
            Ok(effect) => {
                injected.store(true, Ordering::SeqCst);
                ("mid-run (no reboot)".to_string(), effect)
            }
            Err(refused) => {
                return Ok(rejected_report(
                    "turso",
                    shape.slug(),
                    timing.slug(),
                    &refused,
                ));
            }
        },
        CorruptionTiming::PreRestart => {
            env.stop_app()
                .await
                .map_err(|e| SetupFailure(format!("stop_app before corruption: {e:#}")))?;
            let effect = match shape {
                TursoCorruption::TruncateDbFile => {
                    let path = turso_db_path(env);
                    std::fs::write(&path, b"XX")
                        .map_err(|e| SetupFailure(format!("truncate test.db: {e}")))?;
                    format!("truncated {} to 2 bytes", path.display())
                }
                // The SQL shapes need a LIVE handle, so against a stopped app
                // there is no injection at all. Reporting these as anything but
                // rejected is what let 12/12 pre-restart combos look disclosed.
                _ => {
                    return Ok(rejected_report(
                        "turso",
                        shape.slug(),
                        timing.slug(),
                        "SQL shape needs a live handle; no pre-restart form exists",
                    ));
                }
            };
            injected.store(true, Ordering::SeqCst);
            let boot = env.start_app(true).await;
            if boot.is_err() {
                // The reboot surfaced a typed error: the app is NOT running, so
                // driving reads would panic in `test_ctx()` and mask the very
                // fail-loud behaviour this scenario just observed.
                return Ok(ScenarioReport::new(
                    CorruptionOutcome::TypedError,
                    format!(
                        "turso/{}/{}: reboot start_app -> {boot:?} | effect: {effect} | \
                         pre_raw={pre_raw:?} pre_ui={pre_ui:?}",
                        shape.slug(),
                        timing.slug(),
                    ),
                ));
            }
            (format!("reboot start_app -> {boot:?}"), effect)
        }
    };

    let post_raw = count_rows(env, "block_raw").await;
    let post_ui = count_rows(env, "block").await;
    let problems = captured_problems();
    let attributed_problems = attributed(&problems, turso_fault_signatures(shape));

    let outcome = classify(
        &pre_raw,
        &post_raw,
        &post_ui,
        &attributed_problems,
        &boot_note,
    );
    Ok(ScenarioReport::new(
        outcome,
        format!(
            "turso/{}/{}: {boot_note} | effect: {effect} | pre_raw={pre_raw:?} pre_ui={pre_ui:?} \
             post_raw={post_raw:?} post_ui={post_ui:?} | attributed={:?} | ambient_problems={}",
            shape.slug(),
            timing.slug(),
            rendered(&attributed_problems),
            problems.len() - attributed_problems.len(),
        ),
    ))
}

/// Run a full Loro-corruption scenario end to end and record its outcome.
/// See [`run_turso_scenario`] for the `injected` contract.
pub async fn run_loro_scenario(
    env: &mut TestEnvironment,
    shape: LoroCorruption,
    timing: CorruptionTiming,
    injected: &AtomicBool,
) -> Result<ScenarioReport, SetupFailure> {
    let (pre_raw, pre_ui) = setup_seeded_app(env).await?;
    env.wait_for_loro_quiescence(Duration::from_secs(15)).await;
    // Quiescence settles the sync controller but does NOT promise a flush, so
    // force the snapshot to disk — otherwise the injector plants a novel file.
    if let Err(e) = persist_loro_snapshot(env).await {
        return Err(SetupFailure(format!(
            "could not persist Loro snapshot: {e}"
        )));
    }
    // Resolve the snapshot path WHILE RUNNING — `stop_app` clears the store.
    let snap_path = loro_snapshot_path(env).await;
    reset_problems();

    let (boot_note, effect) = match timing {
        CorruptionTiming::MidRun => match damage_loro_file(snap_path.as_deref(), shape) {
            Ok(effect) => {
                injected.store(true, Ordering::SeqCst);
                (
                    "mid-run (in-memory doc unaffected until reload)".to_string(),
                    effect,
                )
            }
            Err(refused) => {
                return Ok(rejected_report(
                    "loro",
                    shape.slug(),
                    timing.slug(),
                    &refused,
                ));
            }
        },
        CorruptionTiming::PreRestart => {
            env.stop_app()
                .await
                .map_err(|e| SetupFailure(format!("stop_app before corruption: {e:#}")))?;
            let effect = match damage_loro_file(snap_path.as_deref(), shape) {
                Ok(effect) => effect,
                Err(refused) => {
                    return Ok(rejected_report(
                        "loro",
                        shape.slug(),
                        timing.slug(),
                        &refused,
                    ));
                }
            };
            injected.store(true, Ordering::SeqCst);
            let boot = env.start_app(true).await;
            if boot.is_err() {
                return Ok(ScenarioReport::new(
                    CorruptionOutcome::TypedError,
                    format!(
                        "loro/{}/{}: reboot start_app -> {boot:?} | effect: {effect} | \
                         pre_raw={pre_raw:?} pre_ui={pre_ui:?}",
                        shape.slug(),
                        timing.slug(),
                    ),
                ));
            }
            (format!("reboot start_app -> {boot:?}"), effect)
        }
    };

    let post_raw = count_rows(env, "block_raw").await;
    let post_ui = count_rows(env, "block").await;
    let problems = captured_problems();
    let attributed_problems = attributed(&problems, LORO_FAULT_SIGNATURES);

    let outcome = classify(
        &pre_raw,
        &post_raw,
        &post_ui,
        &attributed_problems,
        &boot_note,
    );
    Ok(ScenarioReport::new(
        outcome,
        format!(
            "loro/{}/{}: {boot_note} | effect: {effect} | pre_raw={pre_raw:?} pre_ui={pre_ui:?} \
             post_raw={post_raw:?} post_ui={post_ui:?} | attributed={:?} | ambient_problems={}",
            shape.slug(),
            timing.slug(),
            rendered(&attributed_problems),
            problems.len() - attributed_problems.len(),
        ),
    ))
}

/// Substrings that mark a captured problem as CAUSED BY this fault rather than
/// ambient noise. Every `stop_app` floods the collector with "Actor channel
/// closed" / `CacheBlockReader` errors; counting those as disclosure would let
/// any PreRestart shape earn [`CorruptionOutcome::ObservedProblem`] with no
/// corruption injected at all.
/// Tokens name the DAMAGED ARTIFACT, not the subsystem. A subsystem-level token
/// ("loro", "dbsp") matches any message that merely mentions the component —
/// including watchdogs that fire because the vault is empty, which is precisely
/// the loss the guard exists to catch.
fn turso_fault_signatures(shape: TursoCorruption) -> &'static [&'static str] {
    match shape {
        TursoCorruption::DropBlockRawTable => &["block_raw"],
        TursoCorruption::DropDbspStateTable => &["__turso_internal_dbsp_state_v1"],
        TursoCorruption::TruncateDbFile => &["test.db", "short read", "database header"],
    }
}

/// As [`turso_fault_signatures`], for the Loro snapshot shapes — the snapshot
/// FILE, not the Loro subsystem. All three shapes damage the same artifact, so
/// the tokens do not vary by shape.
///
/// NOTE: prod discloses snapshot corruption at WARN (`loro_document_store.rs`,
/// "Corrupted snapshot at …. Recreating."), and the collector captures ERROR
/// only — so an honest run of these shapes yields NO attributed problem.
const LORO_FAULT_SIGNATURES: &[&str] = &["holon_tree", "corrupted snapshot", "snapshot"];

/// Errors that `stop_app` always produces as the actor shuts down. Ambient:
/// they appear with or without a corruption, so no shape may earn a rung from
/// them. Second line of defence behind message-only matching.
const AMBIENT_SHUTDOWN_NOISE: &[&str] = &["actor channel closed", "actor response channel closed"];

/// Keep only the problems attributable to the injected fault: the MESSAGE
/// (never the target — see [`Problem`]) must name the damaged artifact and must
/// not be ambient shutdown noise.
fn attributed<'a>(problems: &'a [Problem], signatures: &[&str]) -> Vec<&'a Problem> {
    problems
        .iter()
        .filter(|p| {
            let msg = p.message.to_lowercase();
            !AMBIENT_SHUTDOWN_NOISE.iter().any(|n| msg.contains(n))
                && signatures.iter().any(|s| msg.contains(&s.to_lowercase()))
        })
        .collect()
}

/// Full `[kind] target (loc): message` lines, for the ledger detail.
/// Attribution never sees these — only [`Problem::message`].
fn rendered<'a>(problems: &[&'a Problem]) -> Vec<&'a str> {
    problems.iter().map(|p| p.rendered.as_str()).collect()
}

/// The oracle. Classify the observed process outcome from the pre/post
/// canonical (`block_raw`) + UI-facing (`block`) counts and the
/// FAULT-ATTRIBUTED problems (see [`attributed`]).
///
/// Precedence, strictest first — the data check is evaluated FIRST and outranks
/// every other rung, because "fewer rows than before" is the observation the
/// forbidden floor is defined by. A disclosure only lifts an observed data loss
/// off the floor when it actually names the damaged artifact; ambient shutdown
/// noise must never launder a silent loss into `ObservedProblem`.
fn classify(
    pre_raw: &Result<i64, String>,
    post_raw: &Result<i64, String>,
    post_ui: &Result<i64, String>,
    attributed_problems: &[&Problem],
    boot_note: &str,
) -> CorruptionOutcome {
    // 1. THE FLOOR, checked before anything can mask it.
    if let (Ok(pre_n), Ok(post_n)) = (pre_raw, post_raw)
        && post_n < pre_n
    {
        return if attributed_problems.is_empty() {
            CorruptionOutcome::SilentDataLoss
        } else {
            // Rows were lost, but the app named the damage: disclosed, not silent.
            CorruptionOutcome::ObservedProblem
        };
    }
    // 2. Fail-loud rungs: a boot or read that returned a typed `Err` faked nothing.
    if boot_note.contains("Err(") || post_raw.is_err() || post_ui.is_err() {
        return CorruptionOutcome::TypedError;
    }
    // 3. No data delta, but the app disclosed damage it attributed to the fault.
    if !attributed_problems.is_empty() {
        return CorruptionOutcome::ObservedProblem;
    }
    CorruptionOutcome::Survived
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a problem the way the collector does: the target is interpolated
    /// into `rendered`, so attribution reading the rendered form sees the
    /// module path too.
    fn problem(target: &str, message: &str) -> Problem {
        Problem {
            message: message.to_string(),
            rendered: format!("[ERROR] {target} (src/x.rs:1): {message}"),
        }
    }

    /// Real ERROR sites in `holon-loro` whose module path contains "loro" but
    /// whose message says nothing about the snapshot. Attributing any of them
    /// would let an unrelated failure vouch for a corrupted snapshot.
    fn loro_targeted_non_snapshot_errors() -> Vec<Problem> {
        vec![
            problem(
                "holon_loro::loro_sync_controller",
                "[LoroSyncController] boot gate never opened — org initial scan may be wedged; \
                 starting the reconcile loop in DISCLOSED degraded mode (it will contend with \
                 the scan)",
            ),
            problem(
                "holon_loro::event_ring",
                "dropping change subscriber: channel full for >5s (stalled consumer) — it will \
                 miss all further changes",
            ),
            problem(
                "holon_loro::loro_blocks_datasource",
                "[LoroBlocksDataSource] Broadcast stream lagged by 7 messages — changes were LOST",
            ),
        ]
    }

    #[test]
    fn loro_module_path_alone_is_never_attribution() {
        for p in loro_targeted_non_snapshot_errors() {
            let got = attributed(std::slice::from_ref(&p), LORO_FAULT_SIGNATURES);
            assert!(
                got.is_empty(),
                "a holon_loro::* ERROR that never names the snapshot must NOT be attributed to a \
                 snapshot corruption, else it vouches for damage it knows nothing about: {}",
                p.rendered
            );
        }
    }

    /// The counterexample that motivated message-only matching: the boot-gate
    /// watchdog is POSITIVELY CORRELATED with an empty vault, so attributing it
    /// would mask exactly the org-derived loss the guard exists to catch.
    #[test]
    fn boot_gate_watchdog_plus_total_row_loss_is_silent_data_loss() {
        let problems = loro_targeted_non_snapshot_errors();
        let attributed_problems = attributed(&problems, LORO_FAULT_SIGNATURES);
        assert!(attributed_problems.is_empty());

        let outcome = classify(
            &Ok(23),
            &Ok(0),
            &Ok(0),
            &attributed_problems,
            "reboot start_app -> Ok(())",
        );
        assert_eq!(
            outcome,
            CorruptionOutcome::SilentDataLoss,
            "23 -> 0 rows with no snapshot-attributed disclosure is the forbidden floor"
        );
    }

    /// The other half of the contract: a message that DOES name the artifact
    /// still earns the disclosed rung, so tightening attribution did not simply
    /// make `ObservedProblem` unreachable.
    #[test]
    fn snapshot_naming_message_still_earns_observed_problem() {
        let problems = vec![problem(
            "holon_loro::loro_document_store",
            "Corrupted snapshot at /tmp/x/.loro/holon_tree.loro: Decode error. Recreating.",
        )];
        let attributed_problems = attributed(&problems, LORO_FAULT_SIGNATURES);
        assert_eq!(attributed_problems.len(), 1);
        assert_eq!(
            classify(&Ok(23), &Ok(0), &Ok(0), &attributed_problems, "ok"),
            CorruptionOutcome::ObservedProblem
        );
    }

    /// Ambient shutdown noise is denied even when it names the artifact.
    #[test]
    fn ambient_shutdown_noise_is_never_attributed() {
        let problems = vec![problem(
            "holon_loro::loro_sync_controller",
            "[LoroSyncController] Outbound reconcile failed: holon_tree snapshot sink write \
             failed: Database error: Actor channel closed",
        )];
        assert!(attributed(&problems, LORO_FAULT_SIGNATURES).is_empty());
    }

    /// Pins the MECHANISM: a token present ONLY in the module path must not
    /// match. `CapturedProblem` interpolates the target into its `Display`, so
    /// reading the rendered form makes every `holon_loro::*` site answer to
    /// "loro" whatever it said. This fails the moment attribution goes back to
    /// the rendered string.
    #[test]
    fn a_token_present_only_in_the_module_path_never_matches() {
        let p = problem(
            "holon_loro::event_ring",
            "dropping change subscriber: channel full for >5s (stalled consumer) — it will miss \
             all further changes",
        );
        assert!(p.rendered.to_lowercase().contains("loro"));
        assert!(!p.message.to_lowercase().contains("loro"));
        assert!(
            attributed(std::slice::from_ref(&p), &["loro"]).is_empty(),
            "attribution matched the module path, not the message: {}",
            p.rendered
        );
    }

    /// Message-only matching is necessary but NOT sufficient, and the reason is
    /// worth pinning: prod prefixes many messages with the component name
    /// (`[LoroSyncController] …`), so a subsystem-level token still matches the
    /// BODY. Only artifact-level tokens keep the boot-gate watchdog — which
    /// fires precisely when the vault is empty — from vouching for a snapshot
    /// it never inspected.
    #[test]
    fn subsystem_tokens_are_unsafe_even_against_the_message() {
        let watchdog = problem(
            "holon_loro::loro_sync_controller",
            "[LoroSyncController] boot gate never opened — org initial scan may be wedged",
        );
        assert_eq!(
            attributed(std::slice::from_ref(&watchdog), &["loro"]).len(),
            1,
            "a bare subsystem token matches the component prefix in the body — do not use one"
        );
        assert!(
            attributed(std::slice::from_ref(&watchdog), LORO_FAULT_SIGNATURES).is_empty(),
            "the shipped artifact-level tokens must not attribute the boot-gate watchdog"
        );
    }

    /// Turso shape #2's token must name the actual system table, not "dbsp".
    #[test]
    fn dbsp_signature_requires_the_real_table_name() {
        let unrelated = vec![problem(
            "holon_turso::ivm",
            "dbsp circuit step failed while the vault was empty",
        )];
        assert!(
            attributed(
                &unrelated,
                turso_fault_signatures(TursoCorruption::DropDbspStateTable)
            )
            .is_empty()
        );
    }
}
