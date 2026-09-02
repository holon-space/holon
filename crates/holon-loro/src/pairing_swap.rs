//! The on-disk half of whole-store pairing: staging, archive, and the swap
//! between them.
//!
//! Pairing replaces this device's global document with the owner's. The
//! replacement is fetched into `<store>/staging-<ts>/` first, so the live store
//! is untouched until a complete owner document exists on disk. The swap is
//! then two renames — the live document into `<store>/archive/<ts>/`, the
//! staged one into its place — bracketed by [`MARKER_NAME`], which names both
//! directories.
//!
//! A process killed anywhere in that sequence leaves the marker behind, and
//! [`complete_interrupted_swap`] decides from the three files which side of the
//! swap the device is on: before the archive rename it rolls back to the
//! pre-pair document, after it rolls forward to the owner's. Neither outcome is
//! an empty store, and a state where both documents are gone is an error naming
//! the archive rather than a silent fresh start.
//!
//! The device-local layout document is not part of a pair and is never moved.

use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::bail;
use serde::Deserialize;
use serde::Serialize;

/// Written before the first rename, removed after the re-import finishes.
pub const MARKER_NAME: &str = "pairing-in-progress.json";

/// Records which owner this device belongs to, written once a pair completes.
pub const RECORD_NAME: &str = "pairing.json";

use crate::loro_document_store::GLOBAL_SNAPSHOT_NAME as GLOBAL_SNAPSHOT;

/// Where an interrupted pair left its two directories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingMarker {
    /// Holds this device's pre-pair global document, and is the only copy of
    /// anything the re-import does not carry.
    pub archive: PathBuf,
    /// Holds the owner's document until the swap moves it into the store.
    pub staging: PathBuf,
    /// The owner's endpoint id, as the invite advertised it.
    pub owner: String,
    pub started_at: String,
}

/// The owner this device is paired to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingRecord {
    pub owner: String,
    pub paired_at: String,
    pub containers: Vec<String>,
    pub archive: PathBuf,
}

pub fn global_snapshot(dir: &Path) -> PathBuf {
    dir.join(GLOBAL_SNAPSHOT)
}

fn marker_path(store_dir: &Path) -> PathBuf {
    store_dir.join(MARKER_NAME)
}

fn record_path(store_dir: &Path) -> PathBuf {
    store_dir.join(RECORD_NAME)
}

/// The owner this device is already paired to, if any.
pub fn read_record(store_dir: &Path) -> anyhow::Result<Option<PairingRecord>> {
    let path = record_path(store_dir);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .with_context(|| format!("{} is not a pairing record", path.display()))
}

pub fn write_record(store_dir: &Path, record: &PairingRecord) -> anyhow::Result<()> {
    let path = record_path(store_dir);
    let json = serde_json::to_vec_pretty(record).context("serializing the pairing record")?;
    std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))
}

/// The marker an interrupted pair left behind.
pub fn read_marker(store_dir: &Path) -> anyhow::Result<Option<PairingMarker>> {
    let path = marker_path(store_dir);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .with_context(|| format!("{} is not a pairing marker", path.display()))
}

pub fn write_marker(store_dir: &Path, marker: &PairingMarker) -> anyhow::Result<()> {
    let path = marker_path(store_dir);
    let json = serde_json::to_vec_pretty(marker).context("serializing the pairing marker")?;
    std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))
}

pub fn remove_marker(store_dir: &Path) -> anyhow::Result<()> {
    let path = marker_path(store_dir);
    if !path.exists() {
        return Ok(());
    }
    std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))
}

/// Move this device's global document into `<store>/archive/<ts>/`, returning
/// that directory. The layout document stays where it is.
pub fn archive_global(store_dir: &Path, stamp: &str) -> anyhow::Result<PathBuf> {
    let dir = store_dir.join("archive").join(stamp);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating the pre-pair archive at {}", dir.display()))?;
    let live = global_snapshot(store_dir);
    if !live.exists() {
        bail!(
            "{} does not exist, so this device has no global document to archive; pairing would \
             adopt over a store that was never written",
            live.display()
        );
    }
    let archived = global_snapshot(&dir);
    std::fs::rename(&live, &archived)
        .with_context(|| format!("archiving {} to {}", live.display(), archived.display()))?;
    Ok(dir)
}

/// Move the staged owner document into the store, then drop the staging
/// directory.
pub fn promote_staged(store_dir: &Path, staging: &Path) -> anyhow::Result<()> {
    let staged = global_snapshot(staging);
    let live = global_snapshot(store_dir);
    std::fs::rename(&staged, &live)
        .with_context(|| format!("promoting {} to {}", staged.display(), live.display()))?;
    std::fs::remove_dir_all(staging)
        .with_context(|| format!("removing the staging directory {}", staging.display()))
}

/// What an interrupted pair still owes.
#[derive(Debug)]
pub enum SwapOutcome {
    /// No pair was in progress, or the one that was had not yet touched the
    /// live document and was rolled back to it.
    Settled,
    /// The owner's document is in place; the content this device wrote before
    /// pairing is still only in `marker.archive` and must be re-imported.
    ReimportOwed(PairingMarker),
}

/// Decide, from the three documents an interrupted pair can leave, whether this
/// store boots as the pre-pair device or as the owner's.
///
/// Runs before anything opens the store, because opening a store whose global
/// document is mid-swap would create a fresh empty one and save over the pair.
pub fn complete_interrupted_swap(store_dir: &Path) -> anyhow::Result<SwapOutcome> {
    let Some(marker) = read_marker(store_dir)? else {
        return Ok(SwapOutcome::Settled);
    };

    let live = global_snapshot(store_dir).exists();
    let staged = global_snapshot(&marker.staging).exists();
    let archived = global_snapshot(&marker.archive).exists();

    if !archived {
        // The archive rename never ran, so the live document is still this
        // device's own and nothing has been adopted.
        if !live {
            bail!(
                "pairing to {} left neither a live global document at {} nor an archived one at \
                 {}; this device's documents are not where the pair recorded them and no store \
                 can be opened without losing content",
                marker.owner,
                global_snapshot(store_dir).display(),
                global_snapshot(&marker.archive).display()
            );
        }
        remove_marker(store_dir)?;
        return Ok(SwapOutcome::Settled);
    }

    if !live {
        if !staged {
            bail!(
                "pairing to {} archived this device's global document to {} but the owner's \
                 document is at neither {} nor {}; recover by moving the archived document back \
                 before opening this store",
                marker.owner,
                global_snapshot(&marker.archive).display(),
                global_snapshot(&marker.staging).display(),
                global_snapshot(store_dir).display()
            );
        }
        promote_staged(store_dir, &marker.staging)?;
    }

    Ok(SwapOutcome::ReimportOwed(marker))
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn marker_for(dir: &Path) -> PairingMarker {
        PairingMarker {
            archive: dir.join("archive").join("stamp"),
            staging: dir.join("staging-stamp"),
            owner: "owner-endpoint".to_string(),
            started_at: "2026-09-03T00:00:00Z".to_string(),
        }
    }

    fn touch(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().expect("a parent directory")).expect("mkdir");
        std::fs::write(path, content).expect("write");
    }

    #[test]
    fn no_marker_leaves_the_store_alone() {
        let dir = TempDir::new().expect("tempdir");
        touch(&global_snapshot(dir.path()), "live");
        assert!(matches!(
            complete_interrupted_swap(dir.path()).expect("swap"),
            SwapOutcome::Settled
        ));
        assert_eq!(
            std::fs::read_to_string(global_snapshot(dir.path())).expect("read"),
            "live"
        );
    }

    #[test]
    fn a_kill_before_the_archive_rename_rolls_back_to_the_pre_pair_document() {
        let dir = TempDir::new().expect("tempdir");
        let marker = marker_for(dir.path());
        touch(&global_snapshot(dir.path()), "phone");
        touch(&global_snapshot(&marker.staging), "owner");
        write_marker(dir.path(), &marker).expect("marker");

        assert!(matches!(
            complete_interrupted_swap(dir.path()).expect("swap"),
            SwapOutcome::Settled
        ));
        assert_eq!(
            std::fs::read_to_string(global_snapshot(dir.path())).expect("read"),
            "phone"
        );
        assert!(read_marker(dir.path()).expect("marker").is_none());
    }

    #[test]
    fn a_kill_between_the_two_renames_finishes_the_promote() {
        let dir = TempDir::new().expect("tempdir");
        let marker = marker_for(dir.path());
        touch(&global_snapshot(&marker.archive), "phone");
        touch(&global_snapshot(&marker.staging), "owner");
        write_marker(dir.path(), &marker).expect("marker");

        let owed = complete_interrupted_swap(dir.path()).expect("swap");
        assert!(matches!(owed, SwapOutcome::ReimportOwed(_)));
        assert_eq!(
            std::fs::read_to_string(global_snapshot(dir.path())).expect("read"),
            "owner"
        );
        assert!(!marker.staging.exists());
    }

    #[test]
    fn a_kill_after_the_promote_still_owes_the_reimport() {
        let dir = TempDir::new().expect("tempdir");
        let marker = marker_for(dir.path());
        touch(&global_snapshot(&marker.archive), "phone");
        touch(&global_snapshot(dir.path()), "owner");
        write_marker(dir.path(), &marker).expect("marker");

        assert!(matches!(
            complete_interrupted_swap(dir.path()).expect("swap"),
            SwapOutcome::ReimportOwed(_)
        ));
    }

    #[test]
    fn a_store_whose_documents_are_both_gone_is_an_error_naming_the_archive() {
        let dir = TempDir::new().expect("tempdir");
        let marker = marker_for(dir.path());
        std::fs::create_dir_all(&marker.archive).expect("mkdir");
        write_marker(dir.path(), &marker).expect("marker");

        let err = complete_interrupted_swap(dir.path()).expect_err("both documents are gone");
        assert!(
            format!("{err:#}").contains(&marker.archive.display().to_string()),
            "the error must name the archive, which is where the content is: {err:#}"
        );
    }
}
