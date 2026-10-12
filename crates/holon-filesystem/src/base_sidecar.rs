//! The merge base of each vault file: the exact bytes Holon last wrote there.
//!
//! The base is device-local, so it lives beside the projection in
//! `<vault>/.holon/bases/` — never in Turso (the base must survive a vault
//! that has no DB) and never in the vault's own tree (a file sync would carry
//! one device's base to another).
//!
//! Each base is keyed by the stable `file` row id, and is used ONLY when its
//! bytes hash to the hash that row recorded. A base whose row has moved on is
//! MISSING, never approximately right.

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use holon_api::EntityUri;

use crate::fs_port::FileSystem;

pub struct BaseSidecars {
    dir: PathBuf,
    fs: Arc<dyn FileSystem>,
}

impl BaseSidecars {
    pub fn new(vault_root: &Path, fs: Arc<dyn FileSystem>) -> Self {
        Self {
            dir: vault_root.join(".holon").join("bases"),
            fs,
        }
    }

    /// The hash a `file` row records for the base it owns. Content only: a
    /// base is still the bytes it was after a renderer change, unlike the
    /// row's content hash, which deliberately invalidates on one.
    pub fn hash(text: &str) -> String {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(text.as_bytes());
        hex::encode(hasher.finalize())
    }

    fn path_of(&self, row: &EntityUri) -> PathBuf {
        self.dir.join(Self::hash(&row.to_string()))
    }

    /// Store `text` as the base of `row`, and return the hash `row` must
    /// record for it. Called BEFORE the row is written, so a crash between the
    /// two leaves a base that fails its hash check instead of one the row
    /// wrongly vouches for.
    pub async fn put(&self, row: &EntityUri, text: &str) -> std::io::Result<String> {
        self.fs.create_dir_all(&self.dir).await?;
        self.fs.write(&self.path_of(row), text.as_bytes()).await?;
        Ok(Self::hash(text))
    }

    /// The base of `row`, or `None` when there is none to trust: no sidecar,
    /// or bytes that do not hash to `expected_hash`.
    pub async fn get(
        &self,
        row: &EntityUri,
        expected_hash: &str,
    ) -> std::io::Result<Option<String>> {
        let text = match self.fs.read_to_string(&self.path_of(row)).await {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        Ok((Self::hash(&text) == expected_hash).then_some(text))
    }

    /// Delete every base that no live `file` row claims. One pass at boot is
    /// enough: an orphan is never read, because reading goes through a row.
    pub async fn sweep(&self, live: &[EntityUri]) -> std::io::Result<usize> {
        if !self.fs.exists(&self.dir) {
            return Ok(0);
        }
        let claimed: HashSet<PathBuf> = live.iter().map(|row| self.path_of(row)).collect();
        let mut swept = 0;
        for file in self.fs.scan_directory(&self.dir).await?.files {
            if !claimed.contains(&file) {
                self.fs.remove(&file).await?;
                swept += 1;
            }
        }
        Ok(swept)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_port::RealFileSystem;

    fn row(id: &str) -> EntityUri {
        EntityUri::file(id)
    }

    #[tokio::test]
    async fn a_base_round_trips_only_under_the_hash_its_row_recorded() {
        let tmp = tempfile::tempdir().unwrap();
        let bases = BaseSidecars::new(tmp.path(), Arc::new(RealFileSystem));
        let a = row("A.org");

        let hash = bases.put(&a, "* base\n").await.unwrap();
        assert_eq!(
            bases.get(&a, &hash).await.unwrap().as_deref(),
            Some("* base\n")
        );
        assert_eq!(
            bases
                .get(&a, "a hash from a row that moved on")
                .await
                .unwrap(),
            None,
            "a base the row does not vouch for must count as missing"
        );
        assert_eq!(
            bases.get(&row("never-written.org"), &hash).await.unwrap(),
            None
        );

        // The sweep must SEE the bases (they live under a dot-directory the
        // vault walker skips) and keep exactly the claimed ones.
        let b = row("B.org");
        bases.put(&b, "* other\n").await.unwrap();
        assert_eq!(bases.sweep(std::slice::from_ref(&a)).await.unwrap(), 1);
        assert_eq!(
            bases.get(&a, &hash).await.unwrap().as_deref(),
            Some("* base\n"),
            "the claimed base must survive its own sweep"
        );
    }
}
