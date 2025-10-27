//! Consolidator epoch marker — fail-loud enforcement of Model.md invariant 10
//! ("consolidator handover is an epoch, not a runtime lookup").
//!
//! Bases (the last-projected snapshots each replica diffs against) are only
//! meaningful relative to ONE consolidator's linear history. Toggling the
//! consolidator mode without re-seeding every base from the new consolidator's
//! state produces two corruption classes:
//!
//!   1. phantom 3-way diffs LWW'd on incomparable timestamps (spurious
//!      rewrites, fake conflicts), and
//!   2. mixed fractional-index keyspaces in the `sort_key` column
//!      (`gen_key_between` values coexisting with Loro-fi values).
//!
//! We persist the consolidator identity at first boot. On later boots a
//! configured-vs-persisted mismatch is a HARD error, unless the operator
//! acknowledges the flip with `HOLON_CONSOLIDATOR_MIGRATE=1`.
//!
//! This module is deliberately component-agnostic: the identity is an opaque
//! [`ConsolidatorId`] derived from the session's pinned `SessionCapabilities`
//! (the single place that resolves "who consolidates"), and the durable state
//! to protect/wipe comes from each component's [`DurableReplicaState`]
//! descriptor — Turso's `-wal`/`-shm` sidecars and Loro's CRDT-dir layout stay
//! in their own crates.

use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use holon_api::capability::ConsolidatorId;
use holon_core::replica_state::DurableReplicaState;

/// Startup guard for Model.md invariant 10. Reads `HOLON_CONSOLIDATOR_MIGRATE`
/// and delegates to [`guard_with_migrate`].
///
/// `marker_dir` is where the epoch marker lives (it must survive the wipe);
/// `components` describe every component's durable footprint — when none of
/// them persist anything, there is no history to protect and the guard skips.
pub fn guard_consolidator_epoch(
    marker_dir: Option<&Path>,
    configured: &ConsolidatorId,
    components: &[&dyn DurableReplicaState],
) -> Result<()> {
    let migrate = std::env::var("HOLON_CONSOLIDATOR_MIGRATE").as_deref() == Ok("1");
    guard_with_migrate(marker_dir, configured, components, migrate)
}

/// Core guard, with the migrate acknowledgement passed explicitly so tests need
/// not mutate process-global env.
fn guard_with_migrate(
    marker_dir: Option<&Path>,
    configured: &ConsolidatorId,
    components: &[&dyn DurableReplicaState],
    migrate: bool,
) -> Result<()> {
    let durable: Vec<_> = components.iter().flat_map(|c| c.durable_paths()).collect();
    if durable.is_empty() {
        tracing::debug!(
            "[consolidator-epoch] no component persists durable state — nothing to protect; \
             skipping invariant-10 guard"
        );
        return Ok(());
    }

    let Some(marker_dir) = marker_dir else {
        bail!(
            "consolidator-epoch guard: components persist durable state ({} path(s)) but no \
             marker location was provided — cannot enforce Model.md invariant 10",
            durable.len()
        );
    };
    let marker_path = marker_dir.join("consolidator");

    if !marker_path.exists() {
        write_marker(&marker_path, configured)?;
        tracing::info!(
            marker = %marker_path.display(),
            mode = %configured,
            "[consolidator-epoch] first boot — recorded consolidator identity"
        );
        return Ok(());
    }

    let persisted = read_marker(&marker_path)?;
    if &persisted == configured {
        return Ok(());
    }

    if migrate {
        tracing::warn!(
            persisted = %persisted,
            configured = %configured,
            "[consolidator-epoch] HOLON_CONSOLIDATOR_MIGRATE=1: wiping every component's durable \
             state so everything re-seeds from the new consolidator. This is the INTERIM \
             wipe-and-reseed; the real state-preserving handover migration is spec 0008 Phase 4.1."
        );
        wipe_durable_state(&durable)?;
        write_marker(&marker_path, configured)?;
        return Ok(());
    }

    bail!(
        "Model.md invariant 10 (consolidator handover is an epoch, not a runtime lookup) \
         violated: the persisted consolidator is `{persisted}` but this process is configured for \
         `{configured}`. Bases are only meaningful against one consolidator's linear history; \
         toggling without re-seeding every base produces (1) phantom 3-way diffs LWW'd on \
         incomparable timestamps (spurious rewrites, fake conflicts) and (2) mixed \
         fractional-index keyspaces in the `sort_key` column (`gen_key_between` values vs \
         Loro-fi). To acknowledge the flip and wipe+reseed from the new consolidator (interim; \
         the real migration is spec 0008 Phase 4.1), set HOLON_CONSOLIDATOR_MIGRATE=1. Marker: \
         {marker}",
        marker = marker_path.display(),
    )
}

fn write_marker(path: &Path, id: &ConsolidatorId) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("create consolidator marker dir {}", dir.display()))?;
    }
    let created_at_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock is before UNIX_EPOCH — cannot stamp consolidator marker")?
        .as_secs();
    let body = format!(
        "consolidator = {}\ncreated_at_unix = {}\n",
        id, created_at_unix
    );
    std::fs::write(path, body)
        .with_context(|| format!("write consolidator marker {}", path.display()))
}

fn read_marker(path: &Path) -> Result<ConsolidatorId> {
    let body = std::fs::read_to_string(path)
        .with_context(|| format!("read consolidator marker {}", path.display()))?;
    let id_line = body
        .lines()
        .find_map(|l| l.strip_prefix("consolidator = "))
        .with_context(|| {
            format!(
                "consolidator marker {} is missing a `consolidator = ` line",
                path.display()
            )
        })?;
    // Opaque by design: an unknown/garbage value is not a parse error, it is a
    // mismatch against the configured id — which produces the invariant-10
    // hard error with both values named.
    Ok(ConsolidatorId::new(id_line.trim()))
}

/// Interim escape hatch: remove every component's durable state so the new
/// consolidator re-seeds from scratch (the Turso db re-seeds from the vault;
/// the CRDT dir holds the Loro doc + the persisted sync-base sidecar). The real
/// state-preserving handover is spec 0008 Phase 4.1.
fn wipe_durable_state(paths: &[std::path::PathBuf]) -> Result<()> {
    for p in paths {
        if !p.exists() {
            continue;
        }
        if p.is_dir() {
            std::fs::remove_dir_all(p)
                .with_context(|| format!("wipe durable state dir {}", p.display()))?;
        } else {
            std::fs::remove_file(p)
                .with_context(|| format!("wipe durable state file {}", p.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use holon_api::capability::SessionCapabilities;
    use holon_loro::durable_state::LoroDurableState;
    use holon_turso::durable_state::TursoDurableState;
    use holon_turso::durable_state::instance_state_root;

    use super::*;

    fn temp() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn projected() -> ConsolidatorId {
        SessionCapabilities::detect_and_pin(true).consolidator_id()
    }

    fn direct() -> ConsolidatorId {
        SessionCapabilities::detect_and_pin(false).consolidator_id()
    }

    struct Setup {
        _dir: tempfile::TempDir,
        db: std::path::PathBuf,
        crdt: std::path::PathBuf,
        marker_dir: std::path::PathBuf,
    }

    fn setup() -> Setup {
        let dir = temp();
        let db = dir.path().join("holon.db");
        std::fs::write(&db, b"db").unwrap();
        let crdt = dir.path().join(".loro");
        std::fs::create_dir_all(&crdt).unwrap();
        let marker_dir = instance_state_root(&db).expect("durable db has a state root");
        Setup {
            db,
            crdt,
            marker_dir,
            _dir: dir,
        }
    }

    fn guard(s: &Setup, configured: &ConsolidatorId, migrate: bool) -> Result<()> {
        let turso = TursoDurableState::new(&s.db);
        let loro = LoroDurableState::new(&s.crdt, true);
        guard_with_migrate(Some(&s.marker_dir), configured, &[&turso, &loro], migrate)
    }

    #[test]
    fn first_boot_writes_marker() {
        let s = setup();
        guard(&s, &projected(), false).expect("first boot");
        let body = std::fs::read_to_string(s.marker_dir.join("consolidator")).unwrap();
        assert!(body.contains("consolidator = projected"), "{body}");
    }

    #[test]
    fn matching_boot_passes() {
        let s = setup();
        guard(&s, &direct(), false).expect("first boot");
        guard(&s, &direct(), false).expect("matching second boot");
    }

    #[test]
    fn mismatched_boot_errors_naming_invariant() {
        let s = setup();
        guard(&s, &direct(), false).expect("first boot direct");
        let err = guard(&s, &projected(), false)
            .expect_err("flip to projected without migrate must error");
        let msg = format!("{err:#}");
        assert!(msg.contains("invariant 10"), "msg: {msg}");
        assert!(
            msg.contains("`direct`") && msg.contains("`projected`"),
            "msg: {msg}"
        );
        assert!(msg.contains("HOLON_CONSOLIDATOR_MIGRATE=1"), "msg: {msg}");
    }

    #[test]
    fn corrupt_marker_value_is_a_mismatch_not_a_parse_error() {
        let s = setup();
        std::fs::create_dir_all(&s.marker_dir).unwrap();
        std::fs::write(
            s.marker_dir.join("consolidator"),
            "consolidator = garbage\n",
        )
        .unwrap();
        let err = guard(&s, &projected(), false).expect_err("garbage id must mismatch");
        let msg = format!("{err:#}");
        assert!(msg.contains("`garbage`"), "msg: {msg}");
        assert!(msg.contains("invariant 10"), "msg: {msg}");
    }

    #[test]
    fn migrate_wipes_and_rewrites() {
        let s = setup();
        guard(&s, &projected(), false).expect("first boot");
        std::fs::write(s.crdt.join("doc.loro"), b"crdt").unwrap();
        for suffix in ["-wal", "-shm"] {
            std::fs::write(format!("{}{}", s.db.display(), suffix), b"sidecar").unwrap();
        }

        guard(&s, &direct(), true).expect("migrate acknowledges the flip");

        assert!(!s.db.exists(), "db wiped");
        assert!(
            !std::path::Path::new(&format!("{}-wal", s.db.display())).exists(),
            "wal wiped"
        );
        assert!(!s.crdt.exists(), "crdt dir wiped");
        let body = std::fs::read_to_string(s.marker_dir.join("consolidator")).unwrap();
        assert!(body.contains("consolidator = direct"), "{body}");
    }

    #[test]
    fn ephemeral_in_memory_db_skips() {
        let turso = TursoDurableState::new(":memory:");
        guard_with_migrate(None, &projected(), &[&turso], false)
            .expect("ephemeral session skips the guard");
    }

    #[test]
    fn durable_state_without_marker_location_fails_loud() {
        let s = setup();
        let turso = TursoDurableState::new(&s.db);
        let err = guard_with_migrate(None, &projected(), &[&turso], false)
            .expect_err("durable state with nowhere to record the epoch must error");
        assert!(format!("{err:#}").contains("invariant 10"));
    }

    #[test]
    fn public_guard_reads_env_and_passes_first_boot() {
        let s = setup();
        let turso = TursoDurableState::new(&s.db);
        let loro = LoroDurableState::new(&s.crdt, true);
        guard_consolidator_epoch(Some(&s.marker_dir), &projected(), &[&turso, &loro])
            .expect("public guard first boot");
    }
}
