//! Foreign-file-syncer tripwire — fail-loud enforcement of Model.md invariant 11
//! ("one consolidator per file replica").
//!
//! Cross-device convergence must travel through Loro/P2P, never through a
//! byte-level file syncer (Syncthing / iCloud / Dropbox) on the vault. When two
//! devices each project the tree to disk and a byte syncer ships those bytes
//! back, each device re-ingests the other's projection as fresh user intent:
//! duplicated ops with wrong attribution, order oscillation, and — the worst
//! case — `.sync-conflict` copies ingested as duplicate-ID documents.
//!
//! We can't stop a syncer, but we can detect its fingerprint: the conflict
//! artifacts it drops into the vault. Startup treats any as a hard error; the
//! watcher skips ingesting an artifact that appears at runtime (disclosed via a
//! `tracing::error!`).

use std::path::{Path, PathBuf};

/// Does this path look like a byte-syncer conflict artifact? Matches the three
/// common syncers' naming schemes:
///
/// - Syncthing: `<base>.sync-conflict-<date>-<time>-<device>.<ext>`
/// - Dropbox:   `<base> (<user>'s conflicted copy <date>).<ext>`
/// - iCloud:    `<base>.<ext>.icloud` placeholder for a not-yet-downloaded file
pub fn is_sync_conflict_artifact(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    name.contains(".sync-conflict-") || name.contains("conflicted copy") || name.ends_with(".icloud")
}

/// All conflict artifacts among `files`, preserving input order.
pub fn find_sync_conflict_artifacts(files: &[PathBuf]) -> Vec<PathBuf> {
    files
        .iter()
        .filter(|p| is_sync_conflict_artifact(p))
        .cloned()
        .collect()
}

/// The hard-error message for a startup scan that found conflict artifacts.
pub fn conflict_artifacts_error(root: &Path, offenders: &[PathBuf]) -> anyhow::Error {
    let list = offenders
        .iter()
        .map(|p| format!("  - {}", p.display()))
        .collect::<Vec<_>>()
        .join("\n");
    anyhow::anyhow!(
        "Model.md invariant 11 (one consolidator per file replica) violated: the vault at {root} \
         contains byte-level file-syncer conflict artifacts, which means it is under a byte syncer \
         (Syncthing / iCloud / Dropbox). That is out of contract — cross-device convergence must \
         travel through Loro/P2P, never a byte syncer, or foreign file writes are re-ingested as \
         duplicate-ID documents with wrong attribution and oscillating order. Move the vault off the \
         synced directory (or exclude it from the syncer) and remove these artifacts:\n{list}",
        root = root.display(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_syncthing_dropbox_icloud() {
        assert!(is_sync_conflict_artifact(Path::new(
            "/v/foo.sync-conflict-20260701-120000-ABCDEF.org"
        )));
        assert!(is_sync_conflict_artifact(Path::new(
            "/v/notes (Martin's conflicted copy 2026-07-01).org"
        )));
        assert!(is_sync_conflict_artifact(Path::new("/v/notes.org.icloud")));
    }

    #[test]
    fn ignores_normal_files() {
        assert!(!is_sync_conflict_artifact(Path::new("/v/notes.org")));
        assert!(!is_sync_conflict_artifact(Path::new("/v/journals/2026.md")));
    }

    #[test]
    fn find_filters_to_offenders() {
        let files = vec![
            PathBuf::from("/v/a.org"),
            PathBuf::from("/v/a.sync-conflict-x.org"),
            PathBuf::from("/v/b.org"),
        ];
        let found = find_sync_conflict_artifacts(&files);
        assert_eq!(found, vec![PathBuf::from("/v/a.sync-conflict-x.org")]);
    }

    // Faithful reproduction of the `FileSyncController::initialize` startup scan:
    // walk a real vault via the `FileSystem` port, then run the tripwire.
    #[tokio::test]
    async fn startup_scan_errors_on_conflict_clean_vault_passes() {
        use crate::fs_port::{FileSystem, RealFileSystem};

        let dir = tempfile::tempdir().unwrap();
        let fs = RealFileSystem;
        fs.write(&dir.path().join("notes.org"), b"* Hi").await.unwrap();

        // Clean vault → no offenders.
        let scanned = fs.scan_directory(dir.path()).await.unwrap();
        assert!(find_sync_conflict_artifacts(&scanned.files).is_empty());

        // Drop a Syncthing conflict artifact → startup must fail loud.
        fs.write(&dir.path().join("foo.sync-conflict-20260701.org"), b"* Dup")
            .await
            .unwrap();
        let scanned = fs.scan_directory(dir.path()).await.unwrap();
        let offenders = find_sync_conflict_artifacts(&scanned.files);
        assert_eq!(offenders.len(), 1);
        let err = conflict_artifacts_error(dir.path(), &offenders);
        let msg = format!("{err:#}");
        assert!(msg.contains("invariant 11"), "msg: {msg}");
        assert!(msg.contains("sync-conflict"), "msg: {msg}");
    }
}
