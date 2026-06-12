//! Persistent device key for iroh sync.
//!
//! A single 32-byte ed25519 secret lives in `<storage_dir>/device.key`.
//! It's loaded on startup, or generated + saved atomically if absent.
//! `stable_peer_id(device_key, shared_tree_id)` is only stable if the
//! key doesn't change between process restarts — without persistence,
//! a rehydrated shared doc would attribute historical writes to one
//! peer and new writes to another, corrupting the CRDT.
//!
//! Atomic write: write to `device.key.tmp`, fsync, rename to final
//! path. Rename is atomic on POSIX; after this the file either exists
//! in its final form or not at all.

use anyhow::{Context, Result, anyhow};
use iroh::SecretKey;
use std::io::Write;
use std::path::Path;

/// Load the device key from `<storage_dir>/device.key`, or generate +
/// persist a new one if the file doesn't exist.
pub fn load_or_create_device_key(storage_dir: &Path) -> Result<SecretKey> {
    let path = storage_dir.join("device.key");
    if path.exists() {
        let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let arr: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
            anyhow!(
                "{} has wrong length: expected 32 bytes, got {}",
                path.display(),
                bytes.len()
            )
        })?;
        Ok(SecretKey::from_bytes(&arr))
    } else {
        let key = SecretKey::generate(&mut rand::rng());
        write_key_atomic(storage_dir, &path, &key.to_bytes())?;
        Ok(key)
    }
}

fn write_key_atomic(storage_dir: &Path, final_path: &Path, bytes: &[u8; 32]) -> Result<()> {
    std::fs::create_dir_all(storage_dir)
        .with_context(|| format!("create dir {}", storage_dir.display()))?;
    let tmp = final_path.with_extension("key.tmp");
    {
        let mut f =
            std::fs::File::create(&tmp).with_context(|| format!("create tmp {}", tmp.display()))?;
        f.write_all(bytes)
            .with_context(|| format!("write tmp {}", tmp.display()))?;
        f.sync_all()
            .with_context(|| format!("fsync tmp {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, final_path)
        .with_context(|| format!("rename {} → {}", tmp.display(), final_path.display()))?;
    // Best-effort fsync of the parent directory so the rename itself
    // is durable. Not all filesystems / platforms honor this, hence
    // best-effort — we don't want to fail a startup on it.
    if let Ok(dir) = std::fs::File::open(storage_dir) {
        let _ = dir.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn generates_key_on_first_call() {
        let dir = TempDir::new().unwrap();
        let key = load_or_create_device_key(dir.path()).unwrap();
        assert!(dir.path().join("device.key").exists());
        // Re-load must return the same bytes.
        let key2 = load_or_create_device_key(dir.path()).unwrap();
        assert_eq!(key.to_bytes(), key2.to_bytes());
    }

    #[test]
    fn rejects_wrong_length_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("device.key"), b"nope").unwrap();
        let err = load_or_create_device_key(dir.path()).unwrap_err();
        assert!(format!("{err}").contains("wrong length"));
    }

    #[test]
    fn creates_missing_storage_dir() {
        let parent = TempDir::new().unwrap();
        let nested = parent.path().join("deep/nested");
        let key = load_or_create_device_key(&nested).unwrap();
        assert!(nested.join("device.key").exists());
        let _ = key; // keep alive until check
    }
}
