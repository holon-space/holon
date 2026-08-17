//! OS-keychain storage for Holon's secret material.
//!
//! Two callers share this seam: the owner-identity seed (ADR 0028 C1/D1) and
//! the OAuth2 credentials an integration sidecar references. A secret filed
//! here is identified by a *service* (fixed per store) and an *account*.
//!
//! - [`KeychainStore`] — the trait every backend implements.
//! - [`platform_keychain`] — the backend for the running platform.
//! - [`UnavailableKeychainStore`] — the **fail-loud** stand-in for platforms
//!   with no wired backend. Every call returns a clear `Err` naming the
//!   platform and the missing precondition; it never silently drops the secret
//!   to disk or memory.
//! - [`InMemoryKeychainStore`] — a test double (never used in production).
//!
//! Secrets are opaque `&[u8]` here. Nothing in this crate logs the secret
//! material.

use anyhow::Result;

#[cfg(target_os = "macos")]
mod mac;
#[cfg(not(target_os = "macos"))]
mod non_mac;

/// Storage seam for secret material. Implementations MUST NOT log the secret.
pub trait KeychainStore: Send + Sync {
    /// Store (or overwrite) the secret for `account`.
    fn store(&self, account: &str, secret: &[u8]) -> Result<()>;
    /// Load the secret for `account`. `Ok(None)` iff no entry exists;
    /// any other backend failure is a loud `Err` (never coerced to `None`).
    fn load(&self, account: &str) -> Result<Option<Vec<u8>>>;
    /// Remove the secret for `account`. Absence is not an error.
    fn delete(&self, account: &str) -> Result<()>;
}

/// Fail-loud stand-in for platforms whose keychain is not wired. Every
/// operation errors, naming the platform and the unmet precondition — the
/// secret is NEVER written in the clear instead. Callers surface the error so
/// the user sees a degraded, non-secret-leaking state.
pub struct UnavailableKeychainStore {
    platform: &'static str,
    precondition: &'static str,
}

impl UnavailableKeychainStore {
    pub fn new() -> Self {
        Self {
            platform: std::env::consts::OS,
            precondition: "no keychain backend is compiled in for this platform",
        }
    }

    /// Android has a keychain backend upstream (`keyring`'s
    /// `android-native-keyring-store`), but it reads the app's JNI handle from
    /// `ndk_context::android_context()`, which no Holon frontend publishes.
    pub fn android_ndk_context_unwired() -> Self {
        Self {
            platform: "android",
            precondition: "ndk_context::android_context() is never initialized by the host app",
        }
    }

    fn refuse<T>(&self, op: &str) -> Result<T> {
        anyhow::bail!(
            "keychain is not implemented on {} yet (op: {op}): {}; refusing to \
             keep the secret in the clear instead",
            self.platform,
            self.precondition
        )
    }
}

impl Default for UnavailableKeychainStore {
    fn default() -> Self {
        Self::new()
    }
}

impl KeychainStore for UnavailableKeychainStore {
    fn store(&self, _: &str, _: &[u8]) -> Result<()> {
        self.refuse("store")
    }
    fn load(&self, _: &str) -> Result<Option<Vec<u8>>> {
        self.refuse("load")
    }
    fn delete(&self, _: &str) -> Result<()> {
        self.refuse("delete")
    }
}

/// The backend for the current platform, filing every secret under `service`.
pub fn platform_keychain(service: &str) -> Box<dyn KeychainStore> {
    #[cfg(target_os = "macos")]
    {
        Box::new(mac::MacKeychainStore::new(service))
    }
    #[cfg(not(target_os = "macos"))]
    {
        non_mac::platform_store(service)
    }
}

/// In-memory keychain for tests ONLY. Never wired into production: it provides
/// no at-rest protection.
#[derive(Default)]
pub struct InMemoryKeychainStore {
    entries: std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>,
}

impl InMemoryKeychainStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl KeychainStore for InMemoryKeychainStore {
    fn store(&self, account: &str, secret: &[u8]) -> Result<()> {
        self.entries
            .lock()
            .unwrap()
            .insert(account.to_string(), secret.to_vec());
        Ok(())
    }
    fn load(&self, account: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.entries.lock().unwrap().get(account).cloned())
    }
    fn delete(&self, account: &str) -> Result<()> {
        self.entries.lock().unwrap().remove(account);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_round_trips() {
        let kc = InMemoryKeychainStore::new();
        assert!(kc.load("owner").unwrap().is_none());
        kc.store("owner", b"secret-bytes").unwrap();
        assert_eq!(kc.load("owner").unwrap().unwrap(), b"secret-bytes");
        kc.delete("owner").unwrap();
        assert!(kc.load("owner").unwrap().is_none());
    }

    #[test]
    fn unavailable_store_fails_loud_never_silently_drops() {
        let kc = UnavailableKeychainStore::new();
        let secret_material = "DEADBEEF-owner-seed-material";
        let err = kc.store("owner", secret_material.as_bytes()).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("not implemented"));
        assert!(msg.contains("refusing"));
        // The secret MATERIAL must never appear in the error text.
        assert!(!msg.contains(secret_material));
        assert!(!msg.contains("DEADBEEF"));
        assert!(kc.load("owner").is_err());
        assert!(kc.delete("owner").is_err());
    }

    #[test]
    fn android_store_names_the_unmet_precondition() {
        let kc = UnavailableKeychainStore::android_ndk_context_unwired();
        let msg = kc.load("owner").unwrap_err().to_string();
        assert!(msg.contains("android"), "{msg}");
        assert!(msg.contains("ndk_context"), "{msg}");
    }
}
