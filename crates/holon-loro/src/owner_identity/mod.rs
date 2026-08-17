//! Owner-identity key: the durable authority root for sharing (ADR 0028
//! D1/OQ4).
//!
//! # Why this is distinct from the device key
//!
//! `device_key` ([`crate::device_key_store`]) is a *transport/session*
//! identity: it authenticates a QUIC connection and seeds per-share CRDT peer
//! ids. It is low-value and stored as a plaintext file beside the vault. The
//! **owner key** is different: it *signs durable authority objects* — the
//! device roster (B1) and the crossing/policy log ([`holon_sharing`]'s
//! `SigningAuthority`). If it leaks, an attacker can mint fleet membership; if
//! it is lost, the owner can no longer enroll devices. It therefore:
//!
//! - lives in the OS keychain, not a plaintext file ([`holon_secrets`]);
//! - has a one-time [`recovery::RecoveryCode`] so a lost founding device is
//!   recoverable;
//! - is generated **lazily** — on the first share/enrollment, never at first
//!   launch (ADR 0028 D1 "prompt DEFERRED to first sharing/enrollment use").
//!
//! Rotation is out of scope here and rides the H4 succession-pointer machinery.
//!
//! # Secret hygiene
//! [`OwnerIdentityKey`] has a redacted `Debug`; its seed never leaves this
//! module except into the keychain. Signatures and public keys are safe to log.

pub mod recovery;

use anyhow::Context;
use anyhow::Result;
use ed25519_dalek::Signer;
use ed25519_dalek::SigningKey;
use ed25519_dalek::VerifyingKey;
use holon_secrets::KeychainStore;
use serde::Deserialize;
use serde::Serialize;

use self::recovery::RecoveryCode;

/// The keychain service every owner-identity secret is filed under.
pub const OWNER_KEYCHAIN_SERVICE: &str = "space.holon.owner-identity";

/// The keychain account the founding device files its owner seed under.
pub const FOUNDING_DEVICE_ACCOUNT: &str = "founding-device";

/// The private owner-identity key. `Debug` is redacted; the 32-byte seed never
/// leaves this module except into the [`KeychainStore`].
#[derive(Clone)]
pub struct OwnerIdentityKey(SigningKey);

impl std::fmt::Debug for OwnerIdentityKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Show the PUBLIC key only — enough to correlate, nothing secret.
        write!(
            f,
            "OwnerIdentityKey(pub={}, seed=<redacted>)",
            self.public()
        )
    }
}

impl OwnerIdentityKey {
    /// Mint a fresh owner key from the CSPRNG. `rand::rng()` is a ChaCha-based
    /// CSPRNG reseeded from the OS — the same source the device key uses.
    pub fn generate() -> Self {
        let mut seed = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rng(), &mut seed);
        let key = SigningKey::from_bytes(&seed);
        seed.fill(0);
        Self(key)
    }

    /// Reconstruct from a 32-byte seed (keychain load / recovery).
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self(SigningKey::from_bytes(seed))
    }

    /// The public half — safe to persist, log, and ship in a roster.
    pub fn public(&self) -> OwnerPublicKey {
        OwnerPublicKey(self.0.verifying_key())
    }

    /// Sign `payload` with the owner key.
    pub fn sign(&self, payload: &[u8]) -> OwnerSignature {
        OwnerSignature(self.0.sign(payload).to_bytes())
    }

    /// The one-time recovery mnemonic for this key. Show once, never persist.
    pub fn recovery_code(&self) -> RecoveryCode {
        RecoveryCode::encode(&self.0.to_bytes())
    }

    /// Seed bytes — for keychain persistence ONLY. Kept crate-private so no
    /// unrelated code path can exfiltrate the secret.
    fn seed_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }
}

/// The public owner-identity key. Safe to log/persist. Compared/serialized as
/// its 32 raw bytes; displayed as hex.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct OwnerPublicKey(VerifyingKey);

impl OwnerPublicKey {
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self> {
        VerifyingKey::from_bytes(bytes)
            .map(OwnerPublicKey)
            .context("owner public key is not a valid Ed25519 point")
    }

    /// Verify `sig` over `payload`. Uses `verify_strict` (rejects
    /// small-order / malleable points). A failure is a loud `Err`.
    pub fn verify(&self, payload: &[u8], sig: &OwnerSignature) -> Result<()> {
        let signature = ed25519_dalek::Signature::from_bytes(&sig.0);
        self.0
            .verify_strict(payload, &signature)
            .context("owner signature verification failed")
    }
}

impl std::fmt::Display for OwnerPublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&hex::encode(self.to_bytes()))
    }
}

impl std::fmt::Debug for OwnerPublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OwnerPublicKey({self})")
    }
}

impl Serialize for OwnerPublicKey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(self.to_bytes()))
    }
}

impl<'de> Deserialize<'de> for OwnerPublicKey {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        let arr: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| serde::de::Error::custom("owner public key must be 32 bytes"))?;
        OwnerPublicKey::from_bytes(&arr).map_err(serde::de::Error::custom)
    }
}

/// An Ed25519 signature by the owner key. Public data — safe to log.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct OwnerSignature([u8; 64]);

impl OwnerSignature {
    pub fn to_bytes(&self) -> [u8; 64] {
        self.0
    }
    pub fn from_bytes(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }
}

impl std::fmt::Debug for OwnerSignature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OwnerSignature({})", hex::encode(self.0))
    }
}

impl Serialize for OwnerSignature {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(self.0))
    }
}

impl<'de> Deserialize<'de> for OwnerSignature {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        let arr: [u8; 64] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| serde::de::Error::custom("owner signature must be 64 bytes"))?;
        Ok(OwnerSignature(arr))
    }
}

/// Custody of the owner key over a [`KeychainStore`]. Encapsulates the D1
/// lifecycle: lazy generation (deferred to first enrollment), keychain
/// persistence, and recovery-code restore.
pub struct OwnerCustody {
    keychain: Box<dyn KeychainStore>,
    account: String,
}

impl OwnerCustody {
    /// Custody over the founding-device account with the platform keychain.
    pub fn founding_device() -> Self {
        Self {
            keychain: holon_secrets::platform_keychain(OWNER_KEYCHAIN_SERVICE),
            account: FOUNDING_DEVICE_ACCOUNT.to_string(),
        }
    }

    /// Custody over an explicit keychain + account (tests / non-default
    /// fleets).
    pub fn with_keychain(keychain: Box<dyn KeychainStore>, account: impl Into<String>) -> Self {
        Self {
            keychain,
            account: account.into(),
        }
    }

    /// Load the owner key if one has already been provisioned. `Ok(None)` means
    /// this device has never founded/recovered a fleet — the caller should run
    /// [`Self::first_enroll`] or [`Self::recover`].
    pub fn load(&self) -> Result<Option<OwnerIdentityKey>> {
        let Some(bytes) = self.keychain.load(&self.account)? else {
            return Ok(None);
        };
        let seed: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
            anyhow::anyhow!(
                "owner seed in keychain for {:?} has wrong length: expected 32, got {}",
                self.account,
                bytes.len()
            )
        })?;
        Ok(Some(OwnerIdentityKey::from_seed(&seed)))
    }

    /// First enrollment on a founding device: generate the owner key, persist
    /// its seed to the keychain, and return it together with the one-time
    /// [`RecoveryCode`] to show the user exactly once.
    ///
    /// Refuses to overwrite an existing owner key — re-founding must go through
    /// an explicit reset, never silently clobber the fleet root.
    pub fn first_enroll(&self) -> Result<(OwnerIdentityKey, RecoveryCode)> {
        if self.keychain.load(&self.account)?.is_some() {
            anyhow::bail!(
                "owner key already provisioned for {:?}; refusing to overwrite \
                 (use recover() on a new device, or an explicit reset)",
                self.account
            );
        }
        let key = OwnerIdentityKey::generate();
        let recovery = key.recovery_code();
        let seed = key.seed_bytes();
        self.keychain
            .store(&self.account, &seed)
            .context("persist freshly-generated owner seed to keychain")?;
        Ok((key, recovery))
    }

    /// Load an existing owner key, or run first-enrollment if none exists.
    /// Returns the recovery code ONLY when a new key was minted (so the caller
    /// knows whether to prompt the user to write it down).
    pub fn load_or_first_enroll(&self) -> Result<(OwnerIdentityKey, Option<RecoveryCode>)> {
        match self.load()? {
            Some(key) => Ok((key, None)),
            None => {
                let (key, recovery) = self.first_enroll()?;
                Ok((key, Some(recovery)))
            }
        }
    }

    /// Recover the owner key on a new founding device from the user's recovery
    /// code, and persist the restored seed to this device's keychain.
    pub fn recover(&self, code: &RecoveryCode) -> Result<OwnerIdentityKey> {
        let seed = code.decode().context("decode recovery code")?;
        let key = OwnerIdentityKey::from_seed(&seed);
        self.keychain
            .store(&self.account, &seed)
            .context("persist recovered owner seed to keychain")?;
        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use holon_secrets::InMemoryKeychainStore;

    use super::*;

    fn custody() -> OwnerCustody {
        OwnerCustody::with_keychain(Box::new(InMemoryKeychainStore::new()), "test-owner")
    }

    #[test]
    fn sign_verify_round_trip() {
        let key = OwnerIdentityKey::generate();
        let pk = key.public();
        let sig = key.sign(b"authorize this");
        assert!(pk.verify(b"authorize this", &sig).is_ok());
    }

    #[test]
    fn verify_rejects_tampered_payload() {
        let key = OwnerIdentityKey::generate();
        let pk = key.public();
        let sig = key.sign(b"grant device A");
        assert!(pk.verify(b"grant device B", &sig).is_err());
    }

    #[test]
    fn verify_rejects_wrong_signer() {
        let a = OwnerIdentityKey::generate();
        let b = OwnerIdentityKey::generate();
        let sig = a.sign(b"payload");
        assert!(b.public().verify(b"payload", &sig).is_err());
    }

    #[test]
    fn public_key_serde_round_trips() {
        let pk = OwnerIdentityKey::generate().public();
        let json = serde_json::to_string(&pk).unwrap();
        let back: OwnerPublicKey = serde_json::from_str(&json).unwrap();
        assert_eq!(pk, back);
    }

    #[test]
    fn signature_serde_round_trips() {
        let key = OwnerIdentityKey::generate();
        let sig = key.sign(b"x");
        let json = serde_json::to_string(&sig).unwrap();
        let back: OwnerSignature = serde_json::from_str(&json).unwrap();
        assert_eq!(sig, back);
        assert!(key.public().verify(b"x", &back).is_ok());
    }

    #[test]
    fn debug_never_leaks_seed() {
        let key = OwnerIdentityKey::generate();
        let dbg = format!("{key:?}");
        assert!(dbg.contains("seed=<redacted>"));
        // The public key hex is fine; the seed hex must not appear.
        let seed_hex = hex::encode(key.seed_bytes());
        assert!(!dbg.contains(&seed_hex));
    }

    #[test]
    fn first_enroll_then_load_recovers_same_key() {
        let c = custody();
        assert!(c.load().unwrap().is_none());
        let (key, _recovery) = c.first_enroll().unwrap();
        let loaded = c.load().unwrap().expect("key present after enroll");
        assert_eq!(key.public(), loaded.public());
    }

    #[test]
    fn first_enroll_refuses_to_overwrite() {
        let c = custody();
        c.first_enroll().unwrap();
        assert!(c.first_enroll().is_err());
    }

    #[test]
    fn recovery_code_restores_identical_key_on_new_device() {
        // Device 1 founds the fleet.
        let c1 = custody();
        let (key1, recovery) = c1.first_enroll().unwrap();

        // Device 2 (fresh keychain) recovers from the code.
        let c2 = OwnerCustody::with_keychain(Box::new(InMemoryKeychainStore::new()), "test-owner");
        let key2 = c2.recover(&recovery).unwrap();

        assert_eq!(key1.public(), key2.public());
        // And a signature by device 2 verifies under device 1's public key.
        let sig = key2.sign(b"same identity");
        assert!(key1.public().verify(b"same identity", &sig).is_ok());
        // Recovery persisted to device 2's keychain.
        assert_eq!(c2.load().unwrap().unwrap().public(), key1.public());
    }

    #[test]
    fn load_or_first_enroll_reports_new_vs_existing() {
        let c = custody();
        let (_k1, first) = c.load_or_first_enroll().unwrap();
        assert!(
            first.is_some(),
            "first call mints and returns recovery code"
        );
        let (_k2, second) = c.load_or_first_enroll().unwrap();
        assert!(second.is_none(), "second call loads, no new recovery code");
    }
}
