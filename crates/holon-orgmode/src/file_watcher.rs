//! File watcher for Org files
//!
//! Watches for changes to .org files and notifies the OrgSyncController.
//! Respects .gitignore (including nested and global gitignore files) and
//! always skips .git/ and .jj/ directories.

use anyhow::Result;
use holon::sync::CanonicalPath;
use ignore::gitignore::Gitignore;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

/// Filesystem entries found by scanning a directory, respecting .gitignore.
pub struct ScannedEntries {
    pub directories: Vec<PathBuf>,
    pub files: Vec<PathBuf>,
}

/// Scan a directory using the `ignore` crate's WalkBuilder.
///
/// Respects .gitignore files and skips hidden directories (.git, .jj, etc.).
/// This is the single source of truth for directory walking in holon-orgmode.
pub fn scan_directory(root: &Path) -> ScannedEntries {
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
        } else if path.extension().is_some_and(|e| e == "org") {
            files.push(path);
        }
    }

    ScannedEntries { directories, files }
}

fn build_gitignore(root: &Path) -> Gitignore {
    let (gitignore, errors) = Gitignore::new(root.join(".gitignore"));
    if let Some(err) = errors {
        warn!("Error parsing .gitignore: {}", err);
    }
    gitignore
}

fn is_ignored(path: &Path, gitignore: &Gitignore) -> bool {
    // Always skip VCS internals
    for component in path.components() {
        let s = component.as_os_str().to_str().unwrap_or("");
        if s == ".git" || s == ".jj" {
            return true;
        }
    }
    let is_dir = path.is_dir();
    gitignore.matched(path, is_dir).is_ignore()
}

/// File watcher for Org files
pub struct OrgFileWatcher {
    watcher: RecommendedWatcher,
    /// Channel sender for file change events
    #[allow(dead_code)]
    change_tx: mpsc::UnboundedSender<PathBuf>,
    /// Channel receiver for file change events
    change_rx: mpsc::UnboundedReceiver<PathBuf>,
    /// Known file hashes for content-based change detection
    known_hashes: Arc<RwLock<HashMap<CanonicalPath, String>>>,
}

impl OrgFileWatcher {
    /// Create a new file watcher
    pub fn new(watch_dir: &Path) -> Result<Self> {
        Self::with_hashes(watch_dir, Arc::new(RwLock::new(HashMap::new())))
    }

    /// Create a new file watcher with shared hash tracking
    pub fn with_hashes(
        watch_dir: &Path,
        known_hashes: Arc<RwLock<HashMap<CanonicalPath, String>>>,
    ) -> Result<Self> {
        let (change_tx, change_rx) = mpsc::unbounded_channel();
        let change_tx_clone = change_tx.clone();
        let gitignore = build_gitignore(watch_dir);

        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                match event.kind {
                    EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_) => {
                        for path in event.paths {
                            if path.extension().map(|e| e == "org").unwrap_or(false)
                                && !is_ignored(&path, &gitignore)
                            {
                                debug!("File change detected: {}", path.display());
                                if let Err(e) = change_tx_clone.send(path) {
                                    warn!("Failed to send file change event: {}", e);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            } else if let Err(e) = res {
                error!("File watcher error: {}", e);
            }
        })?;

        watcher.watch(watch_dir, RecursiveMode::Recursive)?;
        info!("Started watching Org files in: {}", watch_dir.display());

        Ok(Self {
            watcher,
            change_tx,
            change_rx,
            known_hashes,
        })
    }

    /// Compute SHA256 hash of file contents.
    pub fn hash_file(path: &Path) -> std::io::Result<String> {
        crate::file_utils::hash_file(path)
    }

    /// Check if file content actually changed (not just metadata/touch).
    pub async fn content_changed(&self, path: &Path) -> bool {
        let current_hash = match Self::hash_file(path) {
            Ok(h) => h,
            Err(_) => return true, // Assume changed if we can't read
        };

        let canonical_path = CanonicalPath::new(path);
        let known = self.known_hashes.read().await.get(&canonical_path).cloned();

        if Some(&current_hash) != known.as_ref() {
            self.known_hashes
                .write()
                .await
                .insert(canonical_path, current_hash);
            true
        } else {
            false
        }
    }

    /// Update known hash after writing a file (to prevent echo events).
    pub async fn update_hash(&self, path: &Path) -> std::io::Result<()> {
        let hash = Self::hash_file(path)?;
        let canonical_path = CanonicalPath::new(path);
        self.known_hashes.write().await.insert(canonical_path, hash);
        Ok(())
    }

    /// Get a receiver for file change events
    pub fn receiver(&mut self) -> &mut mpsc::UnboundedReceiver<PathBuf> {
        &mut self.change_rx
    }

    /// Consume the watcher and return the receiver
    #[deprecated(note = "This drops the watcher. Use into_parts() instead.")]
    pub fn into_receiver(self) -> mpsc::UnboundedReceiver<PathBuf> {
        self.change_rx
    }

    /// Consume the watcher and return both the watcher and receiver.
    ///
    /// The caller MUST keep the watcher alive for file watching to work.
    pub fn into_parts(
        self,
    ) -> (
        RecommendedWatcher,
        mpsc::UnboundedReceiver<PathBuf>,
        Arc<RwLock<HashMap<CanonicalPath, String>>>,
    ) {
        (self.watcher, self.change_rx, self.known_hashes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_file_watcher_detects_changes() {
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.org");

        let watcher = OrgFileWatcher::new(temp_dir.path()).unwrap();

        sleep(Duration::from_millis(100)).await;

        tokio::fs::write(&test_file, "* Test").await.unwrap();
        sleep(Duration::from_millis(100)).await;

        #[allow(deprecated)]
        let mut receiver = watcher.into_receiver();
        let received = receiver.try_recv();
        assert!(received.is_ok(), "Should receive file change event");
    }

    #[tokio::test]
    async fn test_file_watcher_ignores_git_dir() {
        let temp_dir = TempDir::new().unwrap();
        let git_dir = temp_dir.path().join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        let git_file = git_dir.join("test.org");

        let mut watcher = OrgFileWatcher::new(temp_dir.path()).unwrap();
        sleep(Duration::from_millis(100)).await;

        tokio::fs::write(&git_file, "* Hidden").await.unwrap();
        sleep(Duration::from_millis(200)).await;

        let received = watcher.receiver().try_recv();
        assert!(received.is_err(), "Should NOT receive events from .git/");
    }

    #[tokio::test]
    async fn test_file_watcher_respects_gitignore() {
        let temp_dir = TempDir::new().unwrap();

        // Create .gitignore that ignores "vendor/" directory
        tokio::fs::write(temp_dir.path().join(".gitignore"), "vendor/\n")
            .await
            .unwrap();

        let vendor_dir = temp_dir.path().join("vendor");
        std::fs::create_dir_all(&vendor_dir).unwrap();

        let mut watcher = OrgFileWatcher::new(temp_dir.path()).unwrap();
        sleep(Duration::from_millis(100)).await;

        // Write to ignored path
        tokio::fs::write(vendor_dir.join("dep.org"), "* Vendor dep")
            .await
            .unwrap();
        sleep(Duration::from_millis(200)).await;

        let received = watcher.receiver().try_recv();
        assert!(
            received.is_err(),
            "Should NOT receive events from gitignored paths"
        );
    }
}
