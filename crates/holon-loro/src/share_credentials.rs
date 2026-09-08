//! Custody of the two secrets a gated subtree share needs: the per-share
//! [`CapabilitySecret`] a peer proves possession of at enrollment, and the
//! owner identity key that signs the roster sidecar.
//!
//! # Where the secrets live
//!
//! Both live in the OS keychain ([`KeychainStore`]) and nowhere else. The
//! capability is filed under its own service so a share secret can never be
//! confused with the fleet owner seed, and is keyed by `shared_tree_id` —
//! which is public, so the account name discloses nothing.
//!
//! # Fail loud
//!
//! [`ShareCredentials::load_capability`] returns `Err` when no secret is
//! filed. A gated share whose capability is gone cannot rebuild its roster,
//! and the only safe answer is to refuse — never to fall back to advertising
//! the share un-gated, which is the hole this module exists to close.
//!
//! Nothing here logs secret material: [`CapabilitySecret`] and
//! [`OwnerIdentityKey`] both redact their `Debug`, and no code path formats
//! their bytes.

use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use holon_secrets::KeychainStore;
use tracing::warn;

use crate::degraded_signal_bus::DegradedSignalBus;
use crate::degraded_signal_bus::OWNER_IDENTITY_SUBJECT;
use crate::degraded_signal_bus::ShareDegraded;
use crate::degraded_signal_bus::ShareDegradedReason;
use crate::owner_identity::OwnerCustody;
use crate::owner_identity::OwnerIdentityKey;
use crate::share_enrollment::CapabilitySecret;

/// Keychain service holding one capability secret per share. Distinct from
/// [`crate::owner_identity::OWNER_KEYCHAIN_SERVICE`] so a share secret and the
/// fleet owner seed can never collide on an account name.
pub const SHARE_CAPABILITY_SERVICE: &str = "space.holon.share-capability";

fn capability_account(shared_tree_id: &str) -> String {
    format!("share/{shared_tree_id}")
}

/// The secrets a device holds for the shares it participates in.
pub struct ShareCredentials {
    capabilities: Arc<dyn KeychainStore>,
    owner: OwnerCustody,
}

impl ShareCredentials {
    /// Production custody: the platform keychain for both secrets.
    pub fn platform() -> Self {
        Self {
            capabilities: holon_secrets::platform_keychain(SHARE_CAPABILITY_SERVICE).into(),
            owner: OwnerCustody::founding_device(),
        }
    }

    /// Custody over explicit stores. The capability store is an `Arc` so a
    /// restart test can hand the same store to the backend it builds again.
    pub fn with_stores(capabilities: Arc<dyn KeychainStore>, owner: OwnerCustody) -> Self {
        Self {
            capabilities,
            owner,
        }
    }

    /// Process-memory custody keyed by `namespace`: two constructions with the
    /// same namespace see the same secrets. That is the property the OS
    /// keychain has and a restart test needs — a per-call store would make
    /// every rehydrate look like a lost keychain.
    ///
    /// Test support only. It provides no at-rest protection, which is why it
    /// is compiled out of a production build.
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn in_memory(namespace: &str) -> Self {
        use std::collections::HashMap;
        use std::sync::Mutex;
        use std::sync::OnceLock;

        type Stores = Mutex<HashMap<String, Arc<holon_secrets::InMemoryKeychainStore>>>;
        static STORES: OnceLock<Stores> = OnceLock::new();
        let mut guard = STORES.get_or_init(Default::default).lock().unwrap();
        let store = guard
            .entry(namespace.to_string())
            .or_insert_with(|| Arc::new(holon_secrets::InMemoryKeychainStore::new()))
            .clone();
        Self::with_stores(
            store.clone(),
            OwnerCustody::with_keychain(store, format!("owner/{namespace}")),
        )
    }

    /// The owner key that signs this device's roster sidecars, minting one on
    /// first use (D1 defers generation to the first enrollment).
    ///
    /// A freshly minted key's recovery code has nowhere to go from here — no
    /// surface in the backend can show it to the user — so the mint raises
    /// [`ShareDegradedReason::OwnerRecoveryCodeNotShown`] on `degraded_bus`,
    /// which draws a banner. The bus is a required argument rather than an
    /// optional field precisely so that no caller can obtain an owner key
    /// without giving the disclosure somewhere to land.
    ///
    /// The recovery code itself is dropped here and never logged, carried on
    /// the bus, or rendered.
    pub fn owner_key(&self, degraded_bus: &DegradedSignalBus) -> Result<OwnerIdentityKey> {
        let (key, minted) = self
            .owner
            .load_or_first_enroll()
            .context("load or mint this device's owner identity key")?;
        if minted.is_some() {
            warn!(
                "[share] minted this device's owner identity key on first share; its one-time \
                 recovery code was NOT shown to the user — losing the keychain entry loses the \
                 ability to verify this device's roster sidecars"
            );
            degraded_bus.emit(ShareDegraded {
                shared_tree_id: OWNER_IDENTITY_SUBJECT.to_string(),
                reason: ShareDegradedReason::OwnerRecoveryCodeNotShown,
            });
        }
        Ok(key)
    }

    /// File `secret` as the capability for `shared_tree_id`, replacing any
    /// previous one (a re-accept of the same share re-files the same secret).
    pub fn store_capability(&self, shared_tree_id: &str, secret: &CapabilitySecret) -> Result<()> {
        self.capabilities
            .store(&capability_account(shared_tree_id), secret.as_bytes())
            .with_context(|| format!("persist the capability secret for share {shared_tree_id}"))
    }

    /// The capability for `shared_tree_id`. Absence is an `Err`, not a `None`:
    /// every caller needs the secret to build the share's roster, and the only
    /// alternative to failing here is advertising the share to strangers.
    pub fn load_capability(&self, shared_tree_id: &str) -> Result<CapabilitySecret> {
        let account = capability_account(shared_tree_id);
        let bytes = self
            .capabilities
            .load(&account)
            .with_context(|| format!("read the capability secret for share {shared_tree_id}"))?
            .with_context(|| {
                format!(
                    "no capability secret is filed for share {shared_tree_id}; its roster cannot \
                     be rebuilt and the share must not be advertised un-gated"
                )
            })?;
        let raw: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
            anyhow::anyhow!(
                "the capability secret filed for share {shared_tree_id} has the wrong length: \
                 expected 32 bytes, found {}",
                bytes.len()
            )
        })?;
        Ok(CapabilitySecret::from_bytes(raw))
    }

    /// Drop the capability for `shared_tree_id`. This is what makes an issued
    /// ticket inert: without the secret this device cannot rebuild the roster
    /// the ticket's holder would prove itself against.
    pub fn forget_capability(&self, shared_tree_id: &str) -> Result<()> {
        self.capabilities
            .delete(&capability_account(shared_tree_id))
            .with_context(|| format!("delete the capability secret for share {shared_tree_id}"))
    }
}

#[cfg(test)]
mod tests {
    use holon_secrets::InMemoryKeychainStore;
    use holon_secrets::UnavailableKeychainStore;

    use super::*;

    fn credentials() -> ShareCredentials {
        let keychain = Arc::new(InMemoryKeychainStore::new());
        ShareCredentials::with_stores(
            keychain,
            OwnerCustody::with_keychain(Arc::new(InMemoryKeychainStore::new()), "test-owner"),
        )
    }

    #[test]
    fn a_capability_round_trips_through_the_keychain() {
        let creds = credentials();
        let secret = CapabilitySecret::generate();
        creds.store_capability("tree-1", &secret).unwrap();
        assert_eq!(creds.load_capability("tree-1").unwrap(), secret);
    }

    #[test]
    fn a_missing_capability_is_a_loud_err_not_an_absent_value() {
        let err = credentials()
            .load_capability("tree-never-shared")
            .expect_err("a share with no filed capability must not resolve to a usable roster");
        let msg = format!("{err:#}");
        assert!(msg.contains("tree-never-shared"), "{msg}");
        assert!(msg.contains("un-gated"), "{msg}");
    }

    #[test]
    fn forgetting_a_capability_makes_a_later_load_fail_closed() {
        let creds = credentials();
        creds
            .store_capability("tree-2", &CapabilitySecret::generate())
            .unwrap();
        creds.forget_capability("tree-2").unwrap();
        assert!(creds.load_capability("tree-2").is_err());
    }

    #[test]
    fn a_capability_of_the_wrong_length_is_rejected_rather_than_padded() {
        let keychain = Arc::new(InMemoryKeychainStore::new());
        keychain.store("share/tree-3", &[0u8; 16]).unwrap();
        let creds = ShareCredentials::with_stores(
            keychain,
            OwnerCustody::with_keychain(Arc::new(InMemoryKeychainStore::new()), "test-owner"),
        );
        let msg = format!("{:#}", creds.load_capability("tree-3").unwrap_err());
        assert!(msg.contains("wrong length"), "{msg}");
    }

    #[test]
    fn an_unavailable_keychain_refuses_rather_than_reporting_no_secret() {
        let creds = ShareCredentials::with_stores(
            Arc::new(UnavailableKeychainStore::new()),
            OwnerCustody::with_keychain(Arc::new(InMemoryKeychainStore::new()), "test-owner"),
        );
        let msg = format!("{:#}", creds.load_capability("tree-4").unwrap_err());
        assert!(msg.contains("not implemented"), "{msg}");
    }

    #[test]
    fn the_owner_key_is_minted_once_and_then_reloaded() {
        let creds = credentials();
        let bus = DegradedSignalBus::new();
        let first = creds.owner_key(&bus).unwrap().public();
        assert_eq!(creds.owner_key(&bus).unwrap().public(), first);
    }

    /// The mint drops a one-time recovery code nobody can show, so it is
    /// disclosed as a sticky condition and not only as a log line — and the
    /// disclosure fires on the MINT, not on every later load.
    #[test]
    fn minting_the_owner_key_discloses_that_its_recovery_code_went_unshown() {
        let creds = credentials();
        let bus = DegradedSignalBus::new();
        let mut changes = bus.subscribe().changes;

        creds.owner_key(&bus).unwrap();
        creds.owner_key(&bus).unwrap();

        let mut disclosures: Vec<ShareDegraded> = Vec::new();
        while let Ok(change) = changes.try_recv() {
            if let Some(event) = change.raised()
                && matches!(event.reason, ShareDegradedReason::OwnerRecoveryCodeNotShown)
            {
                disclosures.push(event);
            }
        }
        assert_eq!(
            disclosures.len(),
            1,
            "the mint is disclosed once; reloading the same key discloses nothing new"
        );
        assert_eq!(disclosures[0].shared_tree_id, OWNER_IDENTITY_SUBJECT);
    }

    /// The code is show-once and has no surface, so the only safe thing to do
    /// with it is drop it. Nothing that leaves this module may carry it.
    #[test]
    fn the_recovery_code_never_reaches_the_bus_or_a_log() {
        let creds = credentials();
        let bus = DegradedSignalBus::new();
        let mut changes = bus.subscribe().changes;
        let key = creds.owner_key(&bus).unwrap();

        let code = key.recovery_code();
        let event = changes
            .try_recv()
            .expect("the mint raises exactly one condition")
            .raised()
            .expect("a raise, not a clear");

        // Exact, not a substring probe: the condition is a UNIT variant, so
        // pinning its whole rendering is what proves nothing rides along —
        // adding a field to carry the code would have to break this line.
        assert_eq!(
            format!("{event:?}"),
            format!(
                "ShareDegraded {{ shared_tree_id: {OWNER_IDENTITY_SUBJECT:?}, reason: \
                 OwnerRecoveryCodeNotShown }}"
            )
        );
        assert!(
            format!("{code:?}").contains("redacted"),
            "the code's own Debug must redact, or a `{{:?}}` anywhere leaks it"
        );
    }
}
