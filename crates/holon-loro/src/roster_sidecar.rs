//! C1 roster persistence: an owner-signed `shares/<id>.roster.json` sidecar.
//!
//! # What is persisted, and what is NOT
//!
//! The sidecar holds only **public** roster fields — the share id, the
//! enrollment expiry, the peer cap, the pinned peer fingerprints, the B1
//! owner-signed device entries, and the owner's public key. The **capability
//! secret is NEVER written here** — it lives only in the [`KeychainStore`]
//! (`owner_identity::keychain`). Rehydration reunites the two: keychain secret
//! + verified sidecar body → a live [`ShareRoster`].
//!
//! # Integrity
//!
//! The whole body is signed by the owner key. [`RosterSidecar::load`] verifies
//! that signature under the *expected* owner public key (the one this device
//! loaded from its keychain) and rejects any tamper LOUDLY — a flipped expiry,
//! a smuggled extra peer, or a swapped owner key all fail the check. There is
//! no "load anyway" path: a bad sidecar is an error, not a silently-trusted
//! file.

use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;

use crate::owner_identity::OwnerIdentityKey;
use crate::owner_identity::OwnerPublicKey;
use crate::owner_identity::OwnerSignature;
use crate::share_enrollment::ExpiryTime;
use crate::share_enrollment::PeerFingerprint;
use crate::share_enrollment::ShareRoster;
use crate::share_enrollment::SignedDeviceEntry;

const SIDECAR_VERSION: u32 = 1;

/// The signed, public portion of a share's roster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterSidecarBody {
    pub v: u32,
    pub shared_tree_id: String,
    pub expires_at: ExpiryTime,
    pub max_peers: usize,
    /// Peers already pinned (QUIC-authenticated + admitted). Restored so a
    /// reconnect after restart needs no re-proof.
    pub enrolled: Vec<PeerFingerprint>,
    /// (B1) Owner-signed device-roster entries.
    pub devices: Vec<SignedDeviceEntry>,
    /// The fleet owner's public key this roster trusts.
    pub owner: OwnerPublicKey,
}

impl RosterSidecarBody {
    /// Snapshot a live roster's public state for persistence. `devices` is the
    /// owner-signed B1 entry set (kept alongside the roster by the caller).
    pub fn from_roster(roster: &ShareRoster, devices: Vec<SignedDeviceEntry>) -> Result<Self> {
        let owner = roster.owner().copied().context(
            "cannot persist a roster sidecar without an owner key (owner-signed \
             admission requires it, and the sidecar's integrity is owner-signed)",
        )?;
        Ok(RosterSidecarBody {
            v: SIDECAR_VERSION,
            shared_tree_id: roster.shared_tree_id().to_string(),
            expires_at: roster.expires_at(),
            max_peers: roster.max_peers(),
            enrolled: roster.enrolled_peers().to_vec(),
            devices,
            owner,
        })
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("roster sidecar body is serializable")
    }
}

/// The on-disk sidecar: the public body plus the owner signature over it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RosterSidecar {
    pub body: RosterSidecarBody,
    /// Owner signature over `body`'s canonical bytes. Covers the WHOLE body.
    pub sig: OwnerSignature,
}

impl RosterSidecar {
    /// `<shares_dir>/<shared_tree_id>.roster.json`.
    pub fn path(shares_dir: &Path, shared_tree_id: &str) -> PathBuf {
        shares_dir.join(format!("{shared_tree_id}.roster.json"))
    }

    /// Sign `body` with the owner key and write it atomically.
    pub fn save(
        shares_dir: &Path,
        body: &RosterSidecarBody,
        owner: &OwnerIdentityKey,
    ) -> Result<()> {
        // The body must trust the same owner that signs it — otherwise load
        // (which checks `body.owner == expected`) could never accept it.
        anyhow::ensure!(
            body.owner == owner.public(),
            "roster sidecar body's owner key does not match the signing owner key"
        );
        let sig = owner.sign(&body.canonical_bytes());
        let sidecar = RosterSidecar {
            body: body.clone(),
            sig,
        };
        let json = serde_json::to_vec_pretty(&sidecar).context("serialize roster sidecar")?;
        std::fs::create_dir_all(shares_dir)
            .with_context(|| format!("create shares dir {}", shares_dir.display()))?;
        let final_path = Self::path(shares_dir, &body.shared_tree_id);
        let tmp = final_path.with_extension("json.tmp");
        std::fs::write(&tmp, &json).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &final_path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), final_path.display()))?;
        Ok(())
    }

    /// Load + verify the sidecar for `shared_tree_id`. Fails loudly on:
    /// missing file, unsupported version, an owner key that does not match
    /// `expected_owner`, or an owner signature that does not verify (tamper).
    pub fn load(
        shares_dir: &Path,
        shared_tree_id: &str,
        expected_owner: &OwnerPublicKey,
    ) -> Result<RosterSidecarBody> {
        let path = Self::path(shares_dir, shared_tree_id);
        let bytes = std::fs::read(&path)
            .with_context(|| format!("read roster sidecar {}", path.display()))?;
        let sidecar: RosterSidecar = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse roster sidecar {}", path.display()))?;
        if sidecar.body.v != SIDECAR_VERSION {
            anyhow::bail!(
                "roster sidecar {} has unsupported version {} (this build supports v{SIDECAR_VERSION})",
                path.display(),
                sidecar.body.v
            );
        }
        if &sidecar.body.owner != expected_owner {
            anyhow::bail!(
                "roster sidecar {} is signed by a different owner key than this \
                 device's; refusing to trust it",
                path.display()
            );
        }
        expected_owner
            .verify(&sidecar.body.canonical_bytes(), &sidecar.sig)
            .with_context(|| {
                format!(
                    "roster sidecar {} failed owner-signature verification (tampered on disk)",
                    path.display()
                )
            })?;
        Ok(sidecar.body)
    }

    /// Rehydrate a live [`ShareRoster`] from a verified body plus the
    /// capability secret loaded from the keychain.
    pub fn into_roster(
        body: RosterSidecarBody,
        capability_secret: crate::share_enrollment::CapabilitySecret,
    ) -> ShareRoster {
        ShareRoster::rehydrate(
            body.shared_tree_id,
            capability_secret,
            body.expires_at,
            body.max_peers,
            body.enrolled,
            Some(body.owner),
        )
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::share_enrollment::CapabilitySecret;

    fn owner_and_roster() -> (OwnerIdentityKey, ShareRoster, CapabilitySecret) {
        let owner = OwnerIdentityKey::generate();
        let secret = CapabilitySecret::generate();
        let roster = ShareRoster::new("share-xyz", secret.clone(), ExpiryTime(9_000), 3)
            .with_owner(owner.public());
        (owner, roster, secret)
    }

    #[test]
    fn save_load_round_trips_and_rehydrates() {
        let dir = TempDir::new().unwrap();
        let (owner, roster, secret) = owner_and_roster();
        let device = PeerFingerprint::from_bytes([7u8; 32]);
        let entry = SignedDeviceEntry::sign(&owner, "share-xyz", device, 1234);
        let body = RosterSidecarBody::from_roster(&roster, vec![entry.clone()]).unwrap();
        RosterSidecar::save(dir.path(), &body, &owner).unwrap();

        let loaded = RosterSidecar::load(dir.path(), "share-xyz", &owner.public()).unwrap();
        assert_eq!(loaded, body);

        let mut rehydrated = RosterSidecar::into_roster(loaded, secret);
        // The persisted owner-signed device is admitted after rehydration.
        let authz = rehydrated.authorize_owner_signed(&entry, device).unwrap();
        assert_eq!(authz.peer(), device);
    }

    #[test]
    fn tampered_body_is_rejected_loudly() {
        let dir = TempDir::new().unwrap();
        let (owner, roster, _secret) = owner_and_roster();
        let body = RosterSidecarBody::from_roster(&roster, vec![]).unwrap();
        RosterSidecar::save(dir.path(), &body, &owner).unwrap();

        // Flip the peer cap on disk without re-signing.
        let path = RosterSidecar::path(dir.path(), "share-xyz");
        let raw = std::fs::read_to_string(&path).unwrap();
        let tampered = raw.replace("\"max_peers\": 3", "\"max_peers\": 999");
        assert_ne!(raw, tampered, "test fixture must actually change the file");
        std::fs::write(&path, tampered).unwrap();

        let err = RosterSidecar::load(dir.path(), "share-xyz", &owner.public()).unwrap_err();
        assert!(format!("{err:#}").contains("verification"));
    }

    #[test]
    fn wrong_owner_key_is_rejected() {
        let dir = TempDir::new().unwrap();
        let (owner, roster, _secret) = owner_and_roster();
        let body = RosterSidecarBody::from_roster(&roster, vec![]).unwrap();
        RosterSidecar::save(dir.path(), &body, &owner).unwrap();

        let attacker = OwnerIdentityKey::generate();
        let err = RosterSidecar::load(dir.path(), "share-xyz", &attacker.public()).unwrap_err();
        assert!(format!("{err:#}").contains("different owner key"));
    }

    #[test]
    fn save_refuses_owner_body_mismatch() {
        let dir = TempDir::new().unwrap();
        let (_owner, roster, _secret) = owner_and_roster();
        let body = RosterSidecarBody::from_roster(&roster, vec![]).unwrap();
        let other = OwnerIdentityKey::generate();
        let err = RosterSidecar::save(dir.path(), &body, &other).unwrap_err();
        assert!(format!("{err:#}").contains("does not match"));
    }
}
