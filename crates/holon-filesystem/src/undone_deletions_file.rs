//! The undone deletions of the vault (D229.b R10.1), kept beside the Loro
//! snapshot in `{vault}/.loro/undone-deletions.json`.
//!
//! An undone deletion is a relation between vault files — the user deleted a
//! block from its own file while other files held a copy of it — so it is
//! vault state. The database is derived and rebuilt from the files, and would
//! lose the record on exactly the boots that rebuild it.

use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;

use crate::fs_port::FileSystem;

/// The file's name inside the vault's `.loro` directory.
pub const FILE_NAME: &str = "undone-deletions.json";

/// One undone deletion. Paths are relative to the vault root.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UndoneDeletionRecord {
    /// The block whose deletion Holon undid.
    pub block: String,
    /// The copied heading the block is under.
    pub root: String,
    /// The block's own file.
    pub file: PathBuf,
    /// The files whose copy held the block when the record was written.
    pub copy_files: Vec<PathBuf>,
    /// The fingerprint of the version Holon put back: the deletion removes
    /// only that version.
    pub put_back: String,
}

pub fn path(vault_root: &Path) -> PathBuf {
    vault_root.join(".loro").join(FILE_NAME)
}

/// The records of `vault_root`; none when the file does not exist.
pub async fn load(fs: &dyn FileSystem, vault_root: &Path) -> Result<Vec<UndoneDeletionRecord>> {
    let path = path(vault_root);
    if !fs.exists(&path) {
        return Ok(Vec::new());
    }
    let text = fs
        .read_to_string(&path)
        .await
        .with_context(|| format!("read the undone deletions at {}", path.display()))?;
    serde_json::from_str(&text)
        .with_context(|| format!("parse the undone deletions at {}", path.display()))
}

/// Replace the records of `vault_root` (an atomic write).
pub async fn save(
    fs: &dyn FileSystem,
    vault_root: &Path,
    records: &[UndoneDeletionRecord],
) -> Result<()> {
    let path = path(vault_root);
    let dir = path.parent().expect("the file is inside .loro");
    fs.create_dir_all(dir)
        .await
        .with_context(|| format!("create {}", dir.display()))?;
    let text = serde_json::to_string_pretty(records).expect("records serialize");
    fs.write(&path, text.as_bytes())
        .await
        .with_context(|| format!("write the undone deletions to {}", path.display()))
}
