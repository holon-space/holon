//! Consolidator epoch marker — fail-loud enforcement of Model.md invariant 10
//! ("consolidator handover is an epoch, not a runtime lookup").
//!
//! Bases (the last-projected snapshots each replica diffs against) are only
//! meaningful relative to ONE consolidator's linear history. Toggling Loro on/off
//! without re-seeding every base from the new consolidator's state produces two
//! corruption classes:
//!
//!   1. phantom 3-way diffs LWW'd on incomparable timestamps (spurious rewrites,
//!      fake conflicts), and
//!   2. mixed fractional-index keyspaces in the `sort_key` column
//!      (`gen_key_between` values coexisting with Loro-fi values).
//!
//! We persist the consolidator identity at first boot next to the durable Turso
//! db. On later boots a configured-vs-persisted mismatch is a HARD error, unless
//! the operator acknowledges the flip with `HOLON_CONSOLIDATOR_MIGRATE=1`.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// The consolidator identity recorded in the epoch marker. Model.md invariant 2:
/// exactly one consolidator per vault (Loro when enabled; Turso-LWW in SqlOnly).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsolidatorMode {
    Loro,
    Sql,
}

impl ConsolidatorMode {
    pub fn from_loro_enabled(loro_enabled: bool) -> Self {
        if loro_enabled {
            Self::Loro
        } else {
            Self::Sql
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Loro => "loro",
            Self::Sql => "sql",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        match s {
            "loro" => Ok(Self::Loro),
            "sql" => Ok(Self::Sql),
            other => bail!(
                "consolidator marker records unknown mode {other:?} (expected `loro` or `sql`)"
            ),
        }
    }
}

/// Where the epoch marker lives, or `None` when there is no durable state to
/// protect (ephemeral / in-memory Turso). The marker sits in a `.holon/` dir
/// next to the Turso db so it survives the wipe-and-reseed escape hatch (which
/// removes the db files and the CRDT dir, not `.holon/`).
fn marker_location(db_path: &Path) -> Option<PathBuf> {
    let s = db_path.to_string_lossy();
    if s.is_empty() || s.starts_with(":memory:") {
        return None;
    }
    let parent = db_path.parent()?;
    if parent.as_os_str().is_empty() {
        return None;
    }
    Some(parent.join(".holon").join("consolidator"))
}

/// Startup guard for Model.md invariant 10. Reads `HOLON_CONSOLIDATOR_MIGRATE`
/// and delegates to [`guard_with_migrate`].
pub fn guard_consolidator_epoch(
    db_path: &Path,
    crdt_storage_dir: &Path,
    loro_enabled: bool,
) -> Result<()> {
    let migrate = std::env::var("HOLON_CONSOLIDATOR_MIGRATE").as_deref() == Ok("1");
    guard_with_migrate(db_path, crdt_storage_dir, loro_enabled, migrate)
}

/// Core guard, with the migrate acknowledgement passed explicitly so tests need
/// not mutate process-global env.
fn guard_with_migrate(
    db_path: &Path,
    crdt_storage_dir: &Path,
    loro_enabled: bool,
    migrate: bool,
) -> Result<()> {
    let Some(marker_path) = marker_location(db_path) else {
        tracing::debug!(
            db_path = %db_path.display(),
            "[consolidator-epoch] ephemeral/in-memory Turso — no durable state to protect; \
             skipping invariant-10 guard"
        );
        return Ok(());
    };

    let configured = ConsolidatorMode::from_loro_enabled(loro_enabled);

    if !marker_path.exists() {
        write_marker(&marker_path, configured)?;
        tracing::info!(
            marker = %marker_path.display(),
            mode = configured.as_str(),
            "[consolidator-epoch] first boot — recorded consolidator identity"
        );
        return Ok(());
    }

    let persisted = read_marker(&marker_path)?;
    if persisted == configured {
        return Ok(());
    }

    if migrate {
        tracing::warn!(
            persisted = persisted.as_str(),
            configured = configured.as_str(),
            "[consolidator-epoch] HOLON_CONSOLIDATOR_MIGRATE=1: wiping persisted sync bases + Turso \
             db so everything re-seeds from the new consolidator. This is the INTERIM wipe-and-reseed; \
             the real state-preserving handover migration is spec 0008 Phase 4.1."
        );
        wipe_durable_state(db_path, crdt_storage_dir)?;
        write_marker(&marker_path, configured)?;
        return Ok(());
    }

    bail!(
        "Model.md invariant 10 (consolidator handover is an epoch, not a runtime lookup) violated: \
         the persisted consolidator is `{persisted}` but this process is configured for \
         `{configured}`. Bases are only meaningful against one consolidator's linear history; \
         toggling without re-seeding every base produces (1) phantom 3-way diffs LWW'd on \
         incomparable timestamps (spurious rewrites, fake conflicts) and (2) mixed fractional-index \
         keyspaces in the `sort_key` column (`gen_key_between` values vs Loro-fi). To acknowledge the \
         flip and wipe+reseed from the new consolidator (interim; the real migration is spec 0008 \
         Phase 4.1), set HOLON_CONSOLIDATOR_MIGRATE=1. Marker: {marker}",
        persisted = persisted.as_str(),
        configured = configured.as_str(),
        marker = marker_path.display(),
    )
}

fn write_marker(path: &Path, mode: ConsolidatorMode) -> Result<()> {
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
        mode.as_str(),
        created_at_unix
    );
    std::fs::write(path, body)
        .with_context(|| format!("write consolidator marker {}", path.display()))
}

fn read_marker(path: &Path) -> Result<ConsolidatorMode> {
    let body = std::fs::read_to_string(path)
        .with_context(|| format!("read consolidator marker {}", path.display()))?;
    let mode_line = body
        .lines()
        .find_map(|l| l.strip_prefix("consolidator = "))
        .with_context(|| {
            format!(
                "consolidator marker {} is missing a `consolidator = ` line",
                path.display()
            )
        })?;
    ConsolidatorMode::parse(mode_line.trim())
}

/// Interim escape hatch: remove the durable state tied to the old consolidator
/// so the new one re-seeds from scratch (the Turso db re-seeds from the vault;
/// the CRDT dir holds the Loro doc + the persisted sync-base sidecar). The real
/// state-preserving handover is spec 0008 Phase 4.1.
fn wipe_durable_state(db_path: &Path, crdt_storage_dir: &Path) -> Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let p = if suffix.is_empty() {
            db_path.to_path_buf()
        } else {
            PathBuf::from(format!("{}{}", db_path.display(), suffix))
        };
        if p.exists() {
            std::fs::remove_file(&p)
                .with_context(|| format!("wipe Turso db file {}", p.display()))?;
        }
    }
    if crdt_storage_dir.exists() {
        std::fs::remove_dir_all(crdt_storage_dir)
            .with_context(|| format!("wipe CRDT storage dir {}", crdt_storage_dir.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> tempfile::TempDir {
        tempfile::tempdir().expect("create temp dir")
    }

    #[test]
    fn first_boot_writes_marker() {
        let dir = temp();
        let db = dir.path().join("holon.db");
        let crdt = dir.path().join(".loro");
        guard_with_migrate(&db, &crdt, false, false).expect("first boot succeeds");
        let marker = dir.path().join(".holon").join("consolidator");
        let body = std::fs::read_to_string(&marker).expect("marker written");
        assert!(body.contains("consolidator = sql"), "body: {body}");
        assert!(body.contains("created_at_unix = "), "body: {body}");
    }

    #[test]
    fn matching_boot_passes() {
        let dir = temp();
        let db = dir.path().join("holon.db");
        let crdt = dir.path().join(".loro");
        guard_with_migrate(&db, &crdt, true, false).expect("first boot");
        guard_with_migrate(&db, &crdt, true, false).expect("matching second boot passes");
    }

    #[test]
    fn mismatched_boot_errors_naming_invariant() {
        let dir = temp();
        let db = dir.path().join("holon.db");
        let crdt = dir.path().join(".loro");
        guard_with_migrate(&db, &crdt, false, false).expect("first boot sql");
        let err = guard_with_migrate(&db, &crdt, true, false)
            .expect_err("flip to loro without migrate must error");
        let msg = format!("{err:#}");
        assert!(msg.contains("invariant 10"), "msg: {msg}");
        assert!(msg.contains("`sql`") && msg.contains("`loro`"), "msg: {msg}");
        assert!(msg.contains("HOLON_CONSOLIDATOR_MIGRATE=1"), "msg: {msg}");
    }

    #[test]
    fn migrate_wipes_and_rewrites() {
        let dir = temp();
        let db = dir.path().join("holon.db");
        let crdt = dir.path().join(".loro");
        // First boot as SQL, then simulate durable state existing.
        guard_with_migrate(&db, &crdt, false, false).expect("first boot sql");
        std::fs::write(&db, b"turso-bytes").expect("write db");
        std::fs::write(PathBuf::from(format!("{}-wal", db.display())), b"wal").expect("write wal");
        std::fs::create_dir_all(&crdt).expect("mk crdt dir");
        std::fs::write(crdt.join("holon_tree.loro.sync"), b"base").expect("write sidecar");

        guard_with_migrate(&db, &crdt, true, true).expect("migrate to loro succeeds");

        assert!(!db.exists(), "Turso db wiped");
        assert!(
            !PathBuf::from(format!("{}-wal", db.display())).exists(),
            "wal sidecar wiped"
        );
        assert!(!crdt.exists(), "CRDT dir wiped");
        let marker = dir.path().join(".holon").join("consolidator");
        let body = std::fs::read_to_string(&marker).expect("marker rewritten");
        assert!(body.contains("consolidator = loro"), "body: {body}");
    }

    #[test]
    fn ephemeral_in_memory_db_skips() {
        let dir = temp();
        let crdt = dir.path().join(".loro");
        guard_with_migrate(Path::new(":memory:"), &crdt, true, false)
            .expect("in-memory db skips the guard");
        assert!(
            !dir.path().join(".holon").exists(),
            "no marker written for ephemeral db"
        );
    }

    #[test]
    fn public_guard_reads_env_and_passes_first_boot() {
        let dir = temp();
        let db = dir.path().join("holon.db");
        let crdt = dir.path().join(".loro");
        guard_consolidator_epoch(&db, &crdt, false).expect("public guard first boot");
    }
}
