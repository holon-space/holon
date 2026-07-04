//! `FileSystem` port (ADR 0011): all org/markdown SerDe disk access goes
//! through this trait so tests can substitute an in-memory adapter.
//!
//! Canonicalization is the adapter's responsibility — call sites pass paths
//! as-is and adapters normalise (real fs: `std::fs::canonicalize`, incl.
//! macOS `/var → /private/var` symlink resolution).

use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;

use async_trait::async_trait;

/// Filesystem entries found by scanning a directory.
///
/// `files` contains *all* files (not filtered by extension) — format-specific
/// filters (`.org`, `.md`) belong to the caller.
#[derive(Debug, Default, Clone)]
pub struct ScannedEntries {
    pub directories: Vec<PathBuf>,
    pub files: Vec<PathBuf>,
}

/// Subset of `std::fs::Metadata` the sync path needs (dirty-check signatures).
#[derive(Debug, Clone, Copy)]
pub struct FileMeta {
    pub modified: SystemTime,
    pub len: u64,
}

/// "Where the bytes live" — see ADR 0011.
///
/// `write` is whole-buffer by design: there is no streaming / file-handle
/// API, so the end of a `write` call is the close boundary an in-memory
/// adapter can hook change notifications onto (no partial-write window).
#[async_trait]
pub trait FileSystem: Send + Sync {
    async fn read_to_string(&self, path: &Path) -> std::io::Result<String>;
    async fn read(&self, path: &Path) -> std::io::Result<Vec<u8>>;
    async fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()>;
    /// Remove a file. Errors if absent (like `std::fs::remove_file`). Adapters
    /// with a change-notification surface emit a `Remove` event, mirroring what
    /// the real `notify` watcher delivers for an on-disk deletion.
    async fn remove(&self, path: &Path) -> std::io::Result<()>;
    async fn create_dir_all(&self, path: &Path) -> std::io::Result<()>;
    /// Recursive walk respecting `.gitignore`, skipping hidden entries
    /// (`.git`, `.jj`, …). A missing `root` yields empty entries, not an
    /// error (matches the historical `scan_directory` contract).
    async fn scan_directory(&self, root: &Path) -> std::io::Result<ScannedEntries>;
    async fn metadata(&self, path: &Path) -> std::io::Result<FileMeta>;
    fn exists(&self, path: &Path) -> bool;
    /// Resolve symlinks / normalise. Errors on non-existent paths exactly
    /// like `std::fs::canonicalize` — callers rely on that error to resolve
    /// not-yet-created files via their nearest existing parent.
    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf>;
}

/// Production adapter: thin passthrough to `tokio::fs` / `std::fs`.
pub struct RealFileSystem;

#[async_trait]
impl FileSystem for RealFileSystem {
    async fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        tokio::fs::read_to_string(path).await
    }

    async fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        tokio::fs::read(path).await
    }

    async fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()> {
        tokio::fs::write(path, contents).await
    }

    async fn remove(&self, path: &Path) -> std::io::Result<()> {
        tokio::fs::remove_file(path)
            .await
            .map_err(|e| std::io::Error::new(e.kind(), format!("remove {}: {e}", path.display())))
    }

    async fn create_dir_all(&self, path: &Path) -> std::io::Result<()> {
        tokio::fs::create_dir_all(path).await
    }

    async fn scan_directory(&self, root: &Path) -> std::io::Result<ScannedEntries> {
        let root = root.to_path_buf();
        // `ignore::WalkBuilder` is blocking (gitignore regex DFAs + readdir).
        tokio::task::spawn_blocking(move || walk_directory(&root))
            .await
            .map_err(|e| std::io::Error::other(format!("scan_directory join error: {e}")))
    }

    async fn metadata(&self, path: &Path) -> std::io::Result<FileMeta> {
        let meta = tokio::fs::metadata(path).await?;
        Ok(FileMeta {
            modified: meta.modified()?,
            len: meta.len(),
        })
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        std::fs::canonicalize(path)
    }
}

/// Synchronous gitignore-aware recursive walk — the single source of truth
/// for directory walking (moved here from `holon-orgmode::file_watcher`).
#[tracing::instrument(name = "scan_directory", fields(root = %root.display()))]
pub fn walk_directory(root: &Path) -> ScannedEntries {
    let mut directories = Vec::new();
    let mut files = Vec::new();

    if !root.exists() {
        return ScannedEntries { directories, files };
    }

    for entry in ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .build()
        .flatten()
    {
        let path = entry.into_path();
        if path == root {
            continue;
        }
        if path.is_dir() {
            directories.push(path);
        } else {
            files.push(path);
        }
    }

    ScannedEntries { directories, files }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn real_fs_roundtrip_and_scan() {
        let dir = tempfile::tempdir().unwrap();
        let fs = RealFileSystem;
        let file = dir.path().join("a.org");
        fs.write(&file, b"* Hello").await.unwrap();
        assert_eq!(fs.read_to_string(&file).await.unwrap(), "* Hello");
        assert!(fs.exists(&file));
        let meta = fs.metadata(&file).await.unwrap();
        assert_eq!(meta.len, 7);

        fs.write(&dir.path().join("b.txt"), b"x").await.unwrap();
        let scanned = fs.scan_directory(dir.path()).await.unwrap();
        assert_eq!(scanned.files.len(), 2);

        let missing = fs.scan_directory(&dir.path().join("nope")).await.unwrap();
        assert!(missing.files.is_empty() && missing.directories.is_empty());
    }
}
