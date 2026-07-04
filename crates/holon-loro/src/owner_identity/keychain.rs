//! OS-keychain storage for the owner-identity secret (ADR 0028 C1 / D1).
//!
//! The owner key's 32-byte seed is a first-class secret. It is stored in the
//! platform keychain — never in a plaintext file beside the vault (unlike the
//! transport `device.key`, which is a low-value session identity). This module
//! is the abstracted seam:
//!
//! - [`KeychainStore`] — the trait every backend implements.
//! - [`MacKeychainStore`] — the macOS Keychain Services impl (C1 macOS-first
//!   phasing). Compiled only on macOS.
//! - [`UnavailableKeychainStore`] — the **fail-loud** stand-in for platforms
//!   without a wired keychain yet. Every call returns a clear `Err` naming the
//!   platform; it never silently drops the secret to disk or memory. This is
//!   the disclosed degradation the phasing calls for.
//! - [`InMemoryKeychainStore`] — a test double (never used in production).
//!
//! Secrets are opaque `&[u8]` here; the owner-identity layer decides what the
//! bytes are. Nothing in this module logs the secret material.

use anyhow::Result;

/// The keychain service name every owner-identity secret is filed under.
pub const OWNER_KEYCHAIN_SERVICE: &str = "space.holon.owner-identity";

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

/// macOS Keychain Services backend.
#[cfg(target_os = "macos")]
pub struct MacKeychainStore;

#[cfg(target_os = "macos")]
impl MacKeychainStore {
    /// `errSecItemNotFound` (Security.framework). Hardcoded to avoid taking a
    /// direct dependency on `security-framework-sys`; the value is part of the
    /// stable macOS ABI.
    const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

    pub fn new() -> Self {
        Self
    }
}

#[cfg(target_os = "macos")]
impl Default for MacKeychainStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "macos")]
impl KeychainStore for MacKeychainStore {
    fn store(&self, account: &str, secret: &[u8]) -> Result<()> {
        use anyhow::Context;
        security_framework::passwords::set_generic_password(OWNER_KEYCHAIN_SERVICE, account, secret)
            .with_context(|| {
                format!("store owner secret for account {account:?} in macOS keychain")
            })
    }

    fn load(&self, account: &str) -> Result<Option<Vec<u8>>> {
        match security_framework::passwords::get_generic_password(OWNER_KEYCHAIN_SERVICE, account) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.code() == Self::ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(anyhow::anyhow!(
                "load owner secret for account {account:?} from macOS keychain: {e}"
            )),
        }
    }

    fn delete(&self, account: &str) -> Result<()> {
        match security_framework::passwords::delete_generic_password(
            OWNER_KEYCHAIN_SERVICE,
            account,
        ) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == Self::ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            Err(e) => Err(anyhow::anyhow!(
                "delete owner secret for account {account:?} from macOS keychain: {e}"
            )),
        }
    }
}

/// Fail-loud stand-in for platforms whose keychain is not wired yet (C1 macOS
/// -first phasing). Every operation errors, naming the platform — the owner
/// secret is NEVER silently written to a plaintext file. Callers surface the
/// error so the user sees a degraded, non-secret-leaking state.
pub struct UnavailableKeychainStore {
    platform: &'static str,
}

impl UnavailableKeychainStore {
    pub fn new() -> Self {
        Self {
            platform: std::env::consts::OS,
        }
    }

    fn refuse<T>(&self, op: &str) -> Result<T> {
        anyhow::bail!(
            "owner-identity keychain is not implemented on {} yet (op: {op}); \
             refusing to store the owner secret in the clear — see ADR 0028 C1 \
             (macOS-first phasing)",
            self.platform
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

/// Pick the best keychain backend for the current platform. macOS gets the real
/// keychain; every other platform gets the fail-loud stand-in.
pub fn platform_keychain() -> Box<dyn KeychainStore> {
    #[cfg(target_os = "macos")]
    {
        Box::new(MacKeychainStore::new())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(UnavailableKeychainStore::new())
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
}
