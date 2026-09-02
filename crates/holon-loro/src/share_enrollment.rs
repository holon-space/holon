//! Enrollment / capability authorization for shared-subtree sync.
//!
//! # Why this exists
//!
//! Today a share ticket is a **pure bearer capability whose only secret is the
//! `shared_tree_id`** (see `docs/Reference/SUBTREE_SHARING.md`, B5 + threat
//! model). The acceptor (`iroh_advertiser::accept_loop` →
//! `sync_doc_handle_connection`) authorizes any peer that presents the right
//! ALPN — and the ALPN is `loro-sync/<shared_tree_id>`. The `shared_tree_id`
//! is **not** a secret: it is written to disk (`shares/<id>.loro`, the
//! `.peers.json` sidecar), emitted in tracing (`%shared_tree_id` fields),
//! stored as a SQL `shared-tree-id` property, and observable on the wire in
//! the QUIC Initial-packet ALPN. Anyone who learns it — from a log, a leaked
//! sidecar, or an on-path observer — can **forge** a ticket (they also need a
//! routable `addr`, which likewise leaks) and become a full read/write sync
//! peer, having never seen the real ticket.
//!
//! This module adds the missing authorization layer as a **typed state
//! machine** where the three classic ticket attacks are unrepresentable or
//! rejected loudly:
//!
//! - **Forgery** — access now requires a [`CapabilitySecret`]: 256 bits of
//!   CSPRNG entropy that is *never* derived from any wire- or disk-observable
//!   value, never logged (redacted `Debug`), and proven via a keyed hash — not
//!   the guessable/leaky `shared_tree_id`.
//! - **Replay** — a capability carries an [`ExpiryTime`]; enrollment past it is
//!   rejected. The possession proof is bound to a fresh per-connection
//!   [`Challenge`] the acceptor mints, so a captured proof is useless on the
//!   next connection (different challenge).
//! - **Wrong-peer acceptance** — enrollment pins the peer's iroh node public
//!   key (which QUIC/TLS has *cryptographically* authenticated as
//!   `conn.remote_id()`). A [`ShareRoster`] admits at most `max_peers` distinct
//!   peers; a stranger — or a peer beyond the cap — is rejected. A leaked
//!   capability is thereby bounded to the enrolled device(s), and the overflow
//!   is a *loud* failure the owner sees, not a silent extra reader.
//!
//! # What is deliberately NOT here (design fork — see W4 report)
//!
//! The **enrollment ceremony** (how a capability + expiry are agreed out of
//! band; QR / numeric mutual key comparison; whether the self-device fast path
//! uses an owner-key-signed roster instead of capability-TOFU) and **roster
//! persistence across restart** (a signed sidecar) are NOT decided here. ADR
//! 0028 §H5/C1' rules that ceremony must precede the device fast path but does
//! not fix its transport. This module implements everything *below* that fork:
//! the capability primitive, the proof protocol, and the authorization state
//! machine. Provisioning + persistence are the fork.
//!
//! # Trust model / assumptions (must be checked at ceremony-ruling time)
//!
//! 1. The QUIC channel authenticates the peer's node key (`conn.remote_id()`).
//!    We treat that fingerprint as the peer's identity. iroh's TLS binding is
//!    the crypto root; if it is ever weakened, peer pinning weakens with it.
//! 2. The capability secret reaches the *intended* recipient over a trusted
//!    channel (iMessage/Signal/QR). Tamper/interception of the ticket in
//!    transit is the ceremony's job (mutual verification), not this layer's.
//!    Until the ceremony lands, TOFU enrollment means: whoever presents a valid
//!    capability *first* is enrolled — a capability leaked before first use is
//!    the residual, bounded by `max_peers` and surfaced loudly.
//! 3. Constant-time proof comparison relies on `blake3::Hash`'s documented
//!    constant-time `PartialEq`; we never compare raw secret/tag byte slices
//!    with `==`.

use std::fmt;

use serde::Deserialize;
use serde::Serialize;

use crate::owner_identity::OwnerIdentityKey;
use crate::owner_identity::OwnerPublicKey;
use crate::owner_identity::OwnerSignature;

/// Domain separator folded into the capability id so the public handle can
/// never collide with the proof keyed-hash domain.
const CAPABILITY_ID_DOMAIN: &[u8] = b"holon.share.capability-id.v1";
/// Domain separator for the enrollment possession proof.
const PROOF_DOMAIN: &[u8] = b"holon.share.enrollment-proof.v1";

/// Length-prefix cap for a single enrollment wire frame. Enrollment frames are
/// tiny and fixed-size; anything larger is a malformed/hostile peer.
const MAX_ENROLLMENT_FRAME: usize = 4 * 1024;

fn random_32() -> [u8; 32] {
    use rand::RngCore;
    let mut b = [0u8; 32];
    rand::rng().fill_bytes(&mut b);
    b
}

/// A 256-bit share capability secret. Holding it lets a peer *enroll* into a
/// share; it is the real access secret (unlike the leaky `shared_tree_id`).
///
/// `Debug` is redacted so the secret never lands in a log line or a
/// `format!("{ticket:?}")`. Serialized as URL-safe base64 (no padding) so it
/// travels inside a ticket.
#[derive(Clone, PartialEq, Eq)]
pub struct CapabilitySecret([u8; 32]);

impl CapabilitySecret {
    /// Mint a fresh capability from the CSPRNG.
    pub fn generate() -> Self {
        Self(random_32())
    }

    /// Reconstruct from raw bytes (e.g. after ticket transport).
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The **public** capability id: `blake3(domain || secret)`. Safe to
    /// persist and log — the secret cannot be recovered from it. This is what
    /// the acceptor's roster is keyed on and what the initiator presents to
    /// name *which* capability it is proving, without revealing the secret.
    pub fn capability_id(&self) -> CapabilityId {
        let mut h = blake3::Hasher::new();
        h.update(CAPABILITY_ID_DOMAIN);
        h.update(&self.0);
        CapabilityId(h.finalize())
    }

    /// Compute the possession proof for a specific `challenge` and
    /// `shared_tree_id`: `blake3_keyed(secret, domain || challenge ||
    /// tree_id)`. Binding the tree id stops a proof minted for share A
    /// being replayed to enroll into share B, even under a reused
    /// challenge.
    pub fn prove(&self, challenge: &Challenge, shared_tree_id: &str) -> ProofTag {
        let mut data = Vec::with_capacity(PROOF_DOMAIN.len() + 32 + shared_tree_id.len());
        data.extend_from_slice(PROOF_DOMAIN);
        data.extend_from_slice(&challenge.0);
        data.extend_from_slice(shared_tree_id.as_bytes());
        ProofTag(blake3::keyed_hash(&self.0, &data))
    }

    /// Raw bytes for ticket serialization. Callers must not log the result.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for CapabilitySecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Show only the public id so a share can still be correlated in logs
        // without exposing the secret.
        write!(
            f,
            "CapabilitySecret(<redacted; id={}>)",
            self.capability_id()
        )
    }
}

// Serialize as URL-safe base64 (no padding) so the secret travels inside a
// ticket. Parse-at-the-boundary: deserialization insists on exactly 32 bytes,
// failing loudly on anything else.
impl Serialize for CapabilitySecret {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use base64::Engine as _;
        s.serialize_str(&base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.0))
    }
}

impl<'de> Deserialize<'de> for CapabilitySecret {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use base64::Engine as _;
        use serde::de::Error as _;
        let s = String::deserialize(d)?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(s.trim())
            .map_err(D::Error::custom)?;
        let arr: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
            D::Error::custom(format!(
                "capability secret must be 32 bytes, got {}",
                bytes.len()
            ))
        })?;
        Ok(Self(arr))
    }
}

/// Public, loggable handle for a capability. `blake3::Hash` compares in
/// constant time.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CapabilityId(blake3::Hash);

impl CapabilityId {
    pub fn to_bytes(self) -> [u8; 32] {
        *self.0.as_bytes()
    }
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(blake3::Hash::from_bytes(bytes))
    }
}

impl fmt::Debug for CapabilityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CapabilityId({})", self.0)
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A per-connection acceptor nonce. Freshly minted for every incoming
/// connection and consumed exactly once, so a captured proof cannot be
/// replayed onto a later connection.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Challenge([u8; 32]);

impl Challenge {
    pub fn generate() -> Self {
        Self(random_32())
    }
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for Challenge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Nonce, not a secret, but keep logs terse.
        write!(f, "Challenge(<32b nonce>)")
    }
}

/// The initiator's possession proof. Backed by `blake3::Hash` for
/// constant-time equality.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ProofTag(blake3::Hash);

impl ProofTag {
    pub fn to_bytes(self) -> [u8; 32] {
        *self.0.as_bytes()
    }
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(blake3::Hash::from_bytes(bytes))
    }
}

impl fmt::Debug for ProofTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProofTag(<32b>)")
    }
}

/// Capability expiry, seconds since the Unix epoch. Bounds the *enrollment*
/// window; an already-enrolled peer keeps syncing past expiry (revocation is a
/// separate, forward-only lease concern — ADR 0028 D3/H8, the fork).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ExpiryTime(pub i64);

impl ExpiryTime {
    /// `now` is seconds since the Unix epoch (caller passes
    /// `chrono::Utc::now().timestamp()`; kept as a parameter so the state
    /// machine stays pure and deterministic for tests).
    pub fn is_expired_at(self, now: i64) -> bool {
        now > self.0
    }
}

/// A peer's identity: the 32-byte iroh node public key, which QUIC/TLS has
/// authenticated. This is the pinning key — not any self-asserted value.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PeerFingerprint([u8; 32]);

impl PeerFingerprint {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for PeerFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Short prefix is enough to correlate; full key is not secret but noisy.
        write!(
            f,
            "PeerFingerprint({:02x}{:02x}{:02x}{:02x}…)",
            self.0[0], self.0[1], self.0[2], self.0[3]
        )
    }
}

impl Serialize for PeerFingerprint {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(self.0))
    }
}

impl<'de> Deserialize<'de> for PeerFingerprint {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        let arr: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| serde::de::Error::custom("peer fingerprint must be 32 bytes"))?;
        Ok(PeerFingerprint(arr))
    }
}

/// Loud rejection reasons. Every variant is an authorization *failure*: the
/// caller must surface it and refuse the connection. There is no "allow on
/// doubt" path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthzReject {
    /// A new peer tried to enroll after the capability's expiry.
    Expired { now: i64, expires_at: i64 },
    /// The presented capability id does not match this roster's capability.
    UnknownCapability,
    /// The possession proof did not verify — the peer does not hold the
    /// capability secret (forgery attempt or corruption).
    BadProof,
    /// A new, valid-capability peer arrived but the roster is already at
    /// `max_peers`. Bounds the blast radius of a leaked capability.
    RosterFull { max: usize },
    /// (B1 owner-signed path) The owner signature over the device-roster entry
    /// did not verify — the entry was not signed by this fleet's owner key.
    UnauthorizedDevice,
    /// (B1 owner-signed path) The owner-signed entry authorizes a *different*
    /// device than the one on this connection (fingerprint mismatch). Stops a
    /// captured entry for device X admitting device Y.
    DeviceMismatch,
    /// (B1 owner-signed path) This share has no owner public key configured, so
    /// owner-signed admission is impossible. Fail closed.
    NoOwnerRoster,
}

impl fmt::Display for AuthzReject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthzReject::Expired { now, expires_at } => write!(
                f,
                "share capability expired (now={now}s > expires_at={expires_at}s); \
                 enrollment refused"
            ),
            AuthzReject::UnknownCapability => {
                write!(f, "presented capability id does not match this share")
            }
            AuthzReject::BadProof => write!(
                f,
                "enrollment proof did not verify: peer does not hold the capability secret"
            ),
            AuthzReject::RosterFull { max } => {
                write!(
                    f,
                    "share roster already at capacity ({max} peer(s)); enrollment refused"
                )
            }
            AuthzReject::UnauthorizedDevice => write!(
                f,
                "device-roster entry is not signed by this fleet's owner key; \
                 owner-signed admission refused"
            ),
            AuthzReject::DeviceMismatch => write!(
                f,
                "owner-signed roster entry authorizes a different device than the \
                 connecting peer; admission refused"
            ),
            AuthzReject::NoOwnerRoster => write!(
                f,
                "share has no owner key configured; owner-signed admission is \
                 unavailable (fail closed)"
            ),
        }
    }
}

impl std::error::Error for AuthzReject {}

/// Witness that a peer has been authorized for a share. This type can **only**
/// be constructed by [`ShareRoster::authorize`], so any code path that requires
/// an `&AuthorizedPeer` before syncing cannot be reached with an un-vetted
/// peer — the authorization is proven by the type, not by a convention.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizedPeer {
    peer: PeerFingerprint,
    capability_id: CapabilityId,
    /// True on the connection that first admitted this peer; false on a
    /// subsequent reconnect of an already-enrolled peer.
    newly_enrolled: bool,
}

impl AuthorizedPeer {
    pub fn peer(&self) -> PeerFingerprint {
        self.peer
    }
    pub fn capability_id(&self) -> CapabilityId {
        self.capability_id
    }
    pub fn newly_enrolled(&self) -> bool {
        self.newly_enrolled
    }
}

/// The acceptor-side authorization state for one share. Holds the capability
/// secret (the acceptor minted it) so it can recompute proofs, plus the set of
/// pinned peer fingerprints.
///
/// Illegal states are excluded by construction: there is no way to obtain an
/// [`AuthorizedPeer`] except by presenting a proof that verifies against the
/// stored secret for a live capability under the peer cap.
#[derive(Clone, Debug)]
pub struct ShareRoster {
    shared_tree_id: String,
    capability_id: CapabilityId,
    capability_secret: CapabilitySecret,
    expires_at: ExpiryTime,
    max_peers: usize,
    enrolled: Vec<PeerFingerprint>,
    /// (B1) The fleet owner's public key. When set, this roster ALSO admits
    /// devices the owner has signed into it ([`Self::authorize_owner_signed`]),
    /// independent of the capability-TOFU path. `None` = capability-only share
    /// (a plain third-party share with no self-device fast path).
    owner: Option<OwnerPublicKey>,
}

/// (B1) An owner-signed device-roster entry: proof that the fleet owner
/// authorized `device` to join the share at `added_at`. The signature is over
/// the share id + device fingerprint + timestamp, so an entry minted for one
/// share/device cannot be replayed onto another. Serializable for the C1 signed
/// sidecar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedDeviceEntry {
    /// The QUIC node key the owner authorized. Admission requires the
    /// connecting peer's authenticated fingerprint to equal this.
    pub device: PeerFingerprint,
    /// Unix seconds when the owner added the device (audit / ordering).
    pub added_at: i64,
    /// The owner's signature over [`device_admission_payload`].
    pub sig: OwnerSignature,
}

/// Canonical bytes the owner signs to authorize a device into a share. Binds
/// the share id, the device fingerprint, and the timestamp under a domain
/// separator so a signature is useless outside its exact (share, device) slot.
pub fn device_admission_payload(
    shared_tree_id: &str,
    device: &PeerFingerprint,
    added_at: i64,
) -> Vec<u8> {
    let mut v = Vec::with_capacity(64 + shared_tree_id.len());
    v.extend_from_slice(b"holon.share.device-roster.v1");
    v.push(0);
    v.extend_from_slice(shared_tree_id.as_bytes());
    v.push(0);
    v.extend_from_slice(device.as_bytes());
    v.extend_from_slice(&added_at.to_be_bytes());
    v
}

impl SignedDeviceEntry {
    /// Owner-side: sign `device` into the share named by `shared_tree_id`.
    pub fn sign(
        owner: &OwnerIdentityKey,
        shared_tree_id: &str,
        device: PeerFingerprint,
        added_at: i64,
    ) -> Self {
        let payload = device_admission_payload(shared_tree_id, &device, added_at);
        SignedDeviceEntry {
            device,
            added_at,
            sig: owner.sign(&payload),
        }
    }

    /// Verify this entry against `owner` for `shared_tree_id`. Loud `Err` on
    /// any mismatch.
    pub fn verify(&self, owner: &OwnerPublicKey, shared_tree_id: &str) -> anyhow::Result<()> {
        let payload = device_admission_payload(shared_tree_id, &self.device, self.added_at);
        owner.verify(&payload, &self.sig)
    }
}

impl ShareRoster {
    /// Create a roster for a freshly-minted capability. `max_peers` is clamped
    /// to at least 1 (a roster that can admit nobody is a bug, not a policy).
    pub fn new(
        shared_tree_id: impl Into<String>,
        capability_secret: CapabilitySecret,
        expires_at: ExpiryTime,
        max_peers: usize,
    ) -> Self {
        let capability_id = capability_secret.capability_id();
        Self {
            shared_tree_id: shared_tree_id.into(),
            capability_id,
            capability_secret,
            expires_at,
            max_peers: max_peers.max(1),
            enrolled: Vec::new(),
            owner: None,
        }
    }

    /// Attach the fleet owner's public key, enabling the B1 owner-signed
    /// self-device admission path in addition to capability-TOFU.
    pub fn with_owner(mut self, owner: OwnerPublicKey) -> Self {
        self.owner = Some(owner);
        self
    }

    /// Rebuild a roster from persisted state (C1 sidecar rehydration): the
    /// capability secret comes from the keychain, the public fields + already
    /// -pinned peers come from the verified sidecar body. Restoring the pinned
    /// set is what lets an already-enrolled peer reconnect after a restart
    /// without re-proving.
    pub fn rehydrate(
        shared_tree_id: impl Into<String>,
        capability_secret: CapabilitySecret,
        expires_at: ExpiryTime,
        max_peers: usize,
        enrolled: Vec<PeerFingerprint>,
        owner: Option<OwnerPublicKey>,
    ) -> Self {
        let capability_id = capability_secret.capability_id();
        Self {
            shared_tree_id: shared_tree_id.into(),
            capability_id,
            capability_secret,
            expires_at,
            max_peers: max_peers.max(1),
            enrolled,
            owner,
        }
    }

    pub fn expires_at(&self) -> ExpiryTime {
        self.expires_at
    }
    pub fn max_peers(&self) -> usize {
        self.max_peers
    }
    /// The currently-pinned peer fingerprints (for sidecar persistence).
    pub fn enrolled_peers(&self) -> &[PeerFingerprint] {
        &self.enrolled
    }

    /// The configured owner public key, if this share supports owner-signed
    /// admission.
    pub fn owner(&self) -> Option<&OwnerPublicKey> {
        self.owner.as_ref()
    }

    pub fn shared_tree_id(&self) -> &str {
        &self.shared_tree_id
    }
    pub fn capability_id(&self) -> CapabilityId {
        self.capability_id
    }
    pub fn enrolled_count(&self) -> usize {
        self.enrolled.len()
    }
    pub fn is_enrolled(&self, peer: &PeerFingerprint) -> bool {
        self.enrolled.contains(peer)
    }

    /// Core authorization decision. Ordering matters and is security-relevant:
    ///
    /// 1. An **already-enrolled** peer is admitted immediately — its node key
    ///    is QUIC-authenticated and already pinned, so it needs no fresh proof
    ///    and is unaffected by capability expiry (its lease is separate).
    /// 2. A **new** peer must pass, in order: capability-id match, non-expiry,
    ///    proof verification (constant-time), then the peer cap. The first
    ///    failing check is returned as a loud [`AuthzReject`].
    ///
    /// `now` is Unix seconds. `presented_capability_id` names the capability;
    /// `presented_proof` is the peer's [`ProofTag`] over `challenge` +
    /// `shared_tree_id`. `peer` is the QUIC-authenticated fingerprint.
    pub fn authorize(
        &mut self,
        now: i64,
        challenge: &Challenge,
        presented_capability_id: &CapabilityId,
        presented_proof: &ProofTag,
        peer: PeerFingerprint,
    ) -> Result<AuthorizedPeer, AuthzReject> {
        if self.enrolled.contains(&peer) {
            return Ok(AuthorizedPeer {
                peer,
                capability_id: self.capability_id,
                newly_enrolled: false,
            });
        }

        if presented_capability_id != &self.capability_id {
            return Err(AuthzReject::UnknownCapability);
        }
        if self.expires_at.is_expired_at(now) {
            return Err(AuthzReject::Expired {
                now,
                expires_at: self.expires_at.0,
            });
        }
        let expected = self
            .capability_secret
            .prove(challenge, &self.shared_tree_id);
        // Constant-time compare via `blake3::Hash: PartialEq`.
        if &expected != presented_proof {
            return Err(AuthzReject::BadProof);
        }
        if self.enrolled.len() >= self.max_peers {
            return Err(AuthzReject::RosterFull {
                max: self.max_peers,
            });
        }
        self.enrolled.push(peer);
        Ok(AuthorizedPeer {
            peer,
            capability_id: self.capability_id,
            newly_enrolled: true,
        })
    }

    /// (B1) Owner-signed admission — the self-device fast path. Instead of
    /// proving a capability, the connecting peer presents an owner-signed
    /// [`SignedDeviceEntry`]. Admission requires, in order:
    ///
    /// 1. an already-enrolled peer short-circuits (same as the capability
    ///    path);
    /// 2. this share has an owner key ([`AuthzReject::NoOwnerRoster`]
    ///    otherwise);
    /// 3. the entry authorizes THIS connection's QUIC-authenticated fingerprint
    ///    ([`AuthzReject::DeviceMismatch`] otherwise — a captured entry for
    ///    device X cannot admit device Y);
    /// 4. the owner signature verifies ([`AuthzReject::UnauthorizedDevice`]);
    /// 5. the peer cap is not exceeded.
    ///
    /// Yields the SAME [`AuthorizedPeer`] witness as the capability path, so a
    /// downstream gate that requires `&AuthorizedPeer` treats both admissions
    /// uniformly. Owner-signed devices are NOT subject to capability expiry:
    /// the owner's signature is the authority, not a time-boxed enrollment
    /// window.
    pub fn authorize_owner_signed(
        &mut self,
        entry: &SignedDeviceEntry,
        peer: PeerFingerprint,
    ) -> Result<AuthorizedPeer, AuthzReject> {
        if self.enrolled.contains(&peer) {
            return Ok(AuthorizedPeer {
                peer,
                capability_id: self.capability_id,
                newly_enrolled: false,
            });
        }
        let owner = self.owner.as_ref().ok_or(AuthzReject::NoOwnerRoster)?;
        if entry.device != peer {
            return Err(AuthzReject::DeviceMismatch);
        }
        entry
            .verify(owner, &self.shared_tree_id)
            .map_err(|_| AuthzReject::UnauthorizedDevice)?;
        if self.enrolled.len() >= self.max_peers {
            return Err(AuthzReject::RosterFull {
                max: self.max_peers,
            });
        }
        self.enrolled.push(peer);
        Ok(AuthorizedPeer {
            peer,
            capability_id: self.capability_id,
            newly_enrolled: true,
        })
    }
}

/// The initiator's enrollment message, sent over the (already authenticated,
/// encrypted) QUIC stream: it names the capability and proves possession
/// without transmitting the secret. Fixed 64-byte wire layout:
/// `capability_id(32) || proof(32)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnrollmentProofMsg {
    pub capability_id: CapabilityId,
    pub proof: ProofTag,
}

impl EnrollmentProofMsg {
    pub fn to_wire(self) -> [u8; 64] {
        let mut out = [0u8; 64];
        out[..32].copy_from_slice(&self.capability_id.to_bytes());
        out[32..].copy_from_slice(&self.proof.to_bytes());
        out
    }

    pub fn from_wire(bytes: &[u8]) -> anyhow::Result<Self> {
        if bytes.len() != 64 {
            anyhow::bail!(
                "malformed enrollment proof frame: expected 64 bytes, got {}",
                bytes.len()
            );
        }
        let mut cap = [0u8; 32];
        let mut proof = [0u8; 32];
        cap.copy_from_slice(&bytes[..32]);
        proof.copy_from_slice(&bytes[32..]);
        Ok(Self {
            capability_id: CapabilityId::from_bytes(cap),
            proof: ProofTag::from_bytes(proof),
        })
    }

    /// Build the proof message an initiator sends in response to a challenge.
    pub fn build(
        capability: &CapabilitySecret,
        challenge: &Challenge,
        shared_tree_id: &str,
    ) -> Self {
        Self {
            capability_id: capability.capability_id(),
            proof: capability.prove(challenge, shared_tree_id),
        }
    }
}

// --- iroh wire adapter (thin; the pure state machine above is what the PBTs
// hammer). These are the exact steps the live acceptor/initiator will call once
// the ceremony/roster-persistence fork is ruled. NOT yet driven over a live
// QUIC connection by any test — coverage today is the pure state machine plus
// the `EnrollmentProofMsg` wire round-trip; a live end-to-end test lands
// together with acceptor enforcement. ---

#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
mod wire {
    use std::time::Duration;

    use anyhow::Context;
    use anyhow::Result;
    use tokio::time::timeout;

    use super::*;

    /// Upper bound on any single peer-dependent await in the enrollment
    /// handshake. The handshake is a tiny fixed exchange (challenge → proof →
    /// ack) on a loopback-or-LAN QUIC stream, so a peer that stalls this long
    /// is misbehaving. A bound turns a silent/hostile peer into a loud error
    /// instead of a hang — the same discipline the sync path applies with
    /// `ACCEPT_IO_TIMEOUT`.
    const ENROLL_IO_TIMEOUT: Duration = Duration::from_secs(10);

    /// QUIC application close code the acceptor sets when — and ONLY when —
    /// the roster REFUSED the peer. A dialer cannot see the acceptor's
    /// [`AuthzReject`]; this code is the one deliberate signal that crosses
    /// the wire, so it is what a caller classifies on. Anything else that can
    /// go wrong past `connect()` (an I/O timeout, a stream that never opens, a
    /// malformed frame) closes with [`ENROLLMENT_FAILED_CODE`] instead, so an
    /// infrastructure failure can never be read as a security decision.
    pub const ENROLLMENT_REFUSED_CODE: u32 = 1;

    /// Close code for an enrollment that failed for any reason OTHER than a
    /// roster refusal. Distinct from [`ENROLLMENT_REFUSED_CODE`] on purpose.
    pub const ENROLLMENT_FAILED_CODE: u32 = 2;

    /// Acceptor-side: the roster turned this peer down, carrying WHY.
    /// A typed error rather than a formatted string, so the accept loop
    /// decides the close code by matching a type instead of reading prose.
    #[derive(Debug)]
    pub struct AcceptorRefused(pub AuthzReject);

    impl std::fmt::Display for AcceptorRefused {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "enrollment rejected: {}", self.0)
        }
    }

    impl std::error::Error for AcceptorRefused {}

    /// Initiator-side: the acceptor refused this connection at the enrollment
    /// gate. Carries no reason — the acceptor deliberately does not send one —
    /// so its whole content is "this was a decision, not a failure".
    ///
    /// Attached to the dial error by [`classify_enrollment_failure`]; recover
    /// it with `err.downcast_ref::<EnrollmentRefused>()`.
    #[derive(Debug)]
    pub struct EnrollmentRefused;

    impl std::fmt::Display for EnrollmentRefused {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("the acceptor refused enrollment")
        }
    }

    impl std::error::Error for EnrollmentRefused {}

    /// Decide whether a failed enrollment was the acceptor's REFUSAL or an
    /// infrastructure failure, and tag it so a caller can tell them apart.
    ///
    /// Reads the connection's QUIC close reason — a typed
    /// `ConnectionError::ApplicationClosed` carrying the code the acceptor
    /// set — never the text of `err`. Every failure past `connect()` already
    /// travels under the same "enrollment" context, so a substring test
    /// cannot separate a refusal from a 10s timeout.
    ///
    /// A connection with no application close reason is NOT a refusal: an
    /// unclassifiable failure must stay loud rather than be absorbed as a
    /// security decision.
    pub fn classify_enrollment_failure(
        conn: &iroh::endpoint::Connection,
        err: anyhow::Error,
    ) -> anyhow::Error {
        let refused = matches!(
            conn.close_reason(),
            Some(iroh::endpoint::ConnectionError::ApplicationClosed(ref frame))
                if frame.error_code == ENROLLMENT_REFUSED_CODE.into()
        );
        if refused {
            anyhow::Error::new(EnrollmentRefused).context(format!("{err:#}"))
        } else {
            err
        }
    }

    async fn bounded<F, T>(what: &str, fut: F) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>>,
    {
        match timeout(ENROLL_IO_TIMEOUT, fut).await {
            Ok(r) => r,
            Err(_) => anyhow::bail!("enrollment {what} timed out after {ENROLL_IO_TIMEOUT:?}"),
        }
    }

    async fn write_frame(stream: &mut iroh::endpoint::SendStream, data: &[u8]) -> Result<()> {
        if data.len() > MAX_ENROLLMENT_FRAME {
            anyhow::bail!(
                "refusing to send oversize enrollment frame: {} bytes",
                data.len()
            );
        }
        let len = (data.len() as u32).to_be_bytes();
        stream.write_all(&len).await?;
        stream.write_all(data).await?;
        Ok(())
    }

    async fn read_frame(stream: &mut iroh::endpoint::RecvStream) -> Result<Vec<u8>> {
        let mut len_buf = [0u8; 4];
        stream
            .read_exact(&mut len_buf)
            .await
            .context("read enrollment frame length")?;
        let len = u32::from_be_bytes(len_buf) as usize;
        if len > MAX_ENROLLMENT_FRAME {
            anyhow::bail!("peer sent oversize enrollment frame: {len} bytes");
        }
        let mut data = vec![0u8; len];
        stream
            .read_exact(&mut data)
            .await
            .context("read enrollment frame body")?;
        Ok(data)
    }

    /// Extract the QUIC-authenticated peer fingerprint from a connection.
    pub fn peer_fingerprint(conn: &iroh::endpoint::Connection) -> PeerFingerprint {
        PeerFingerprint::from_bytes(*conn.remote_id().as_bytes())
    }

    /// Acceptor side: mint a challenge, read the initiator's proof, and
    /// authorize against `roster`. Runs on its own dedicated bi-stream that the
    /// *acceptor* opens: challenge-response is acceptor-speaks-first, and QUIC
    /// only surfaces a freshly-opened bidi stream to the peer once the opener
    /// transmits — so the side that writes first (here, the challenge) must be
    /// the side that opens, else `accept_bi`/`open_bi` mutually block. The sync
    /// stream that follows is opened by the initiator (which writes first
    /// there), so the two streams never collide. Returns the loud
    /// [`AuthzReject`] (wrapped) on any failure so the caller drops the
    /// connection without ever reaching the sync primitive.
    pub async fn acceptor_enroll(
        conn: &iroh::endpoint::Connection,
        roster: &mut ShareRoster,
        now: i64,
    ) -> Result<AuthorizedPeer> {
        let (mut send, mut recv) = bounded("open stream", async {
            conn.open_bi()
                .await
                .map_err(|e| anyhow::anyhow!("open enrollment stream: {e}"))
        })
        .await?;
        let challenge = Challenge::generate();
        bounded(
            "send challenge",
            write_frame(&mut send, challenge.as_bytes()),
        )
        .await
        .context("send enrollment challenge")?;
        let proof_bytes = bounded("read proof", read_frame(&mut recv))
            .await
            .context("read enrollment proof")?;
        let msg = EnrollmentProofMsg::from_wire(&proof_bytes)?;
        let peer = peer_fingerprint(conn);
        let authorized = roster
            .authorize(now, &challenge, &msg.capability_id, &msg.proof, peer)
            .map_err(|reject| anyhow::Error::new(AcceptorRefused(reject)))?;
        // Ack so the initiator knows the sync stream may proceed.
        bounded("send ack", write_frame(&mut send, b"OK"))
            .await
            .context("send enrollment ack")?;
        send.finish().context("finish enrollment ack stream")?;
        Ok(authorized)
    }

    /// Initiator side: accept the acceptor-opened enrollment stream, read the
    /// challenge, prove possession, await the ack. Runs to completion before
    /// the initiator opens the sync stream (see `acceptor_enroll` for why
    /// the acceptor is the opener here).
    pub async fn initiator_enroll(
        conn: &iroh::endpoint::Connection,
        capability: &CapabilitySecret,
        shared_tree_id: &str,
    ) -> Result<()> {
        let (mut send, mut recv) = bounded("accept stream", async {
            conn.accept_bi()
                .await
                .map_err(|e| anyhow::anyhow!("accept enrollment stream: {e}"))
        })
        .await?;
        let challenge_bytes = bounded("read challenge", read_frame(&mut recv))
            .await
            .context("read enrollment challenge")?;
        if challenge_bytes.len() != 32 {
            anyhow::bail!(
                "malformed enrollment challenge: expected 32 bytes, got {}",
                challenge_bytes.len()
            );
        }
        let mut c = [0u8; 32];
        c.copy_from_slice(&challenge_bytes);
        let challenge = Challenge::from_bytes(c);
        let msg = EnrollmentProofMsg::build(capability, &challenge, shared_tree_id);
        bounded("send proof", write_frame(&mut send, &msg.to_wire()))
            .await
            .context("send enrollment proof")?;
        send.finish().context("finish enrollment proof stream")?;
        let ack = bounded("read ack", read_frame(&mut recv))
            .await
            .context("read enrollment ack")?;
        if ack != b"OK" {
            anyhow::bail!("acceptor did not acknowledge enrollment");
        }
        Ok(())
    }
}

#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub use wire::AcceptorRefused;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub use wire::ENROLLMENT_FAILED_CODE;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub use wire::ENROLLMENT_REFUSED_CODE;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub use wire::EnrollmentRefused;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub use wire::acceptor_enroll;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub use wire::classify_enrollment_failure;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub use wire::initiator_enroll;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub use wire::peer_fingerprint;

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(n: u8) -> PeerFingerprint {
        PeerFingerprint::from_bytes([n; 32])
    }

    fn roster(max: usize, expires_at: i64) -> (ShareRoster, CapabilitySecret) {
        let cap = CapabilitySecret::generate();
        let r = ShareRoster::new("tree-abc", cap.clone(), ExpiryTime(expires_at), max);
        (r, cap)
    }

    // --- B1 owner-signed device roster admission ---

    fn owner_roster(max: usize) -> (OwnerIdentityKey, ShareRoster) {
        let owner = OwnerIdentityKey::generate();
        let cap = CapabilitySecret::generate();
        let r =
            ShareRoster::new("tree-abc", cap, ExpiryTime(10_000), max).with_owner(owner.public());
        (owner, r)
    }

    #[test]
    fn owner_signed_device_is_admitted_and_idempotent() {
        let (owner, mut r) = owner_roster(2);
        let device = peer(5);
        let entry = SignedDeviceEntry::sign(&owner, "tree-abc", device, 42);
        let a = r
            .authorize_owner_signed(&entry, device)
            .expect("owner-signed device admitted");
        assert!(a.newly_enrolled());
        assert_eq!(r.enrolled_count(), 1);
        // Reconnect: idempotent, no re-verify.
        let a2 = r.authorize_owner_signed(&entry, device).unwrap();
        assert!(!a2.newly_enrolled());
        assert_eq!(r.enrolled_count(), 1);
    }

    #[test]
    fn entry_for_other_device_cannot_admit_this_peer() {
        let (owner, mut r) = owner_roster(2);
        // Owner signed device 5, but device 6 connects presenting that entry.
        let entry = SignedDeviceEntry::sign(&owner, "tree-abc", peer(5), 42);
        assert_eq!(
            r.authorize_owner_signed(&entry, peer(6)).unwrap_err(),
            AuthzReject::DeviceMismatch
        );
    }

    #[test]
    fn entry_signed_by_wrong_owner_is_rejected() {
        let (_owner, mut r) = owner_roster(2);
        let attacker = OwnerIdentityKey::generate();
        let device = peer(7);
        let forged = SignedDeviceEntry::sign(&attacker, "tree-abc", device, 42);
        assert_eq!(
            r.authorize_owner_signed(&forged, device).unwrap_err(),
            AuthzReject::UnauthorizedDevice
        );
    }

    #[test]
    fn entry_for_other_share_is_rejected() {
        let (owner, mut r) = owner_roster(2);
        let device = peer(8);
        // Owner signed this device, but into a DIFFERENT share id.
        let cross_share = SignedDeviceEntry::sign(&owner, "tree-other", device, 42);
        assert_eq!(
            r.authorize_owner_signed(&cross_share, device).unwrap_err(),
            AuthzReject::UnauthorizedDevice
        );
    }

    #[test]
    fn owner_signed_admission_without_owner_key_fails_closed() {
        let (mut r, _cap) = roster(2, 10_000); // no owner configured
        let owner = OwnerIdentityKey::generate();
        let device = peer(9);
        let entry = SignedDeviceEntry::sign(&owner, "tree-abc", device, 42);
        assert_eq!(
            r.authorize_owner_signed(&entry, device).unwrap_err(),
            AuthzReject::NoOwnerRoster
        );
    }

    #[test]
    fn owner_signed_respects_peer_cap() {
        let (owner, mut r) = owner_roster(1);
        let e5 = SignedDeviceEntry::sign(&owner, "tree-abc", peer(5), 1);
        let e6 = SignedDeviceEntry::sign(&owner, "tree-abc", peer(6), 2);
        r.authorize_owner_signed(&e5, peer(5)).unwrap();
        assert_eq!(
            r.authorize_owner_signed(&e6, peer(6)).unwrap_err(),
            AuthzReject::RosterFull { max: 1 }
        );
    }

    #[test]
    fn signed_device_entry_serde_round_trips() {
        let owner = OwnerIdentityKey::generate();
        let entry = SignedDeviceEntry::sign(&owner, "tree-abc", peer(3), 99);
        let json = serde_json::to_string(&entry).unwrap();
        let back: SignedDeviceEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
        assert!(back.verify(&owner.public(), "tree-abc").is_ok());
    }

    #[test]
    fn honest_enrollment_succeeds_and_is_idempotent() {
        let (mut r, cap) = roster(1, 10_000);
        let ch = Challenge::generate();
        let msg = EnrollmentProofMsg::build(&cap, &ch, "tree-abc");
        let a = r
            .authorize(100, &ch, &msg.capability_id, &msg.proof, peer(1))
            .expect("honest peer authorized");
        assert!(a.newly_enrolled());
        assert_eq!(r.enrolled_count(), 1);
        // Re-auth of the same peer: idempotent, no new proof needed, survives
        // a fresh challenge.
        let ch2 = Challenge::generate();
        let a2 = r
            .authorize(100, &ch2, &msg.capability_id, &msg.proof, peer(1))
            .expect("enrolled peer re-authorized");
        assert!(!a2.newly_enrolled());
        assert_eq!(r.enrolled_count(), 1);
    }

    #[test]
    fn forged_capability_is_rejected() {
        let (mut r, _cap) = roster(1, 10_000);
        // Attacker mints their own capability (does not hold the real secret).
        let forged = CapabilitySecret::generate();
        let ch = Challenge::generate();
        let msg = EnrollmentProofMsg::build(&forged, &ch, "tree-abc");
        let err = r
            .authorize(100, &ch, &msg.capability_id, &msg.proof, peer(9))
            .unwrap_err();
        assert_eq!(err, AuthzReject::UnknownCapability);
        assert_eq!(r.enrolled_count(), 0);
    }

    #[test]
    fn right_capability_wrong_proof_is_rejected() {
        let (mut r, cap) = roster(1, 10_000);
        let ch = Challenge::generate();
        // Correct capability id, but proof computed over a DIFFERENT challenge
        // (a replayed/forged proof).
        let stale = Challenge::generate();
        let proof = cap.prove(&stale, "tree-abc");
        let err = r
            .authorize(100, &ch, &cap.capability_id(), &proof, peer(9))
            .unwrap_err();
        assert_eq!(err, AuthzReject::BadProof);
        assert_eq!(r.enrolled_count(), 0);
    }

    #[test]
    fn proof_bound_to_tree_id() {
        // A proof minted for a different tree id must not verify here.
        let (mut r, cap) = roster(1, 10_000);
        let ch = Challenge::generate();
        let proof_other_tree = cap.prove(&ch, "tree-OTHER");
        let err = r
            .authorize(100, &ch, &cap.capability_id(), &proof_other_tree, peer(9))
            .unwrap_err();
        assert_eq!(err, AuthzReject::BadProof);
    }

    #[test]
    fn expired_capability_rejects_new_peer() {
        let (mut r, cap) = roster(1, 500);
        let ch = Challenge::generate();
        let msg = EnrollmentProofMsg::build(&cap, &ch, "tree-abc");
        let err = r
            .authorize(1_000, &ch, &msg.capability_id, &msg.proof, peer(1))
            .unwrap_err();
        assert!(matches!(err, AuthzReject::Expired { .. }));
    }

    #[test]
    fn expiry_does_not_evict_already_enrolled_peer() {
        let (mut r, cap) = roster(1, 500);
        let ch = Challenge::generate();
        let msg = EnrollmentProofMsg::build(&cap, &ch, "tree-abc");
        r.authorize(100, &ch, &msg.capability_id, &msg.proof, peer(1))
            .expect("enrolled before expiry");
        // Long after expiry, the enrolled peer still authorizes.
        let ch2 = Challenge::generate();
        let a = r
            .authorize(10_000, &ch2, &msg.capability_id, &msg.proof, peer(1))
            .expect("enrolled peer keeps access past expiry");
        assert!(!a.newly_enrolled());
    }

    #[test]
    fn roster_full_rejects_extra_peer() {
        let (mut r, cap) = roster(1, 10_000);
        let ch = Challenge::generate();
        let msg = EnrollmentProofMsg::build(&cap, &ch, "tree-abc");
        r.authorize(100, &ch, &msg.capability_id, &msg.proof, peer(1))
            .expect("first peer");
        // A DIFFERENT valid-capability peer is refused once the cap is hit —
        // this bounds a leaked capability to `max_peers` devices, loudly.
        let ch2 = Challenge::generate();
        let msg2 = EnrollmentProofMsg::build(&cap, &ch2, "tree-abc");
        let err = r
            .authorize(100, &ch2, &msg2.capability_id, &msg2.proof, peer(2))
            .unwrap_err();
        assert_eq!(err, AuthzReject::RosterFull { max: 1 });
    }

    #[test]
    fn wire_roundtrip() {
        let cap = CapabilitySecret::generate();
        let ch = Challenge::generate();
        let msg = EnrollmentProofMsg::build(&cap, &ch, "tree-abc");
        let round = EnrollmentProofMsg::from_wire(&msg.to_wire()).unwrap();
        assert_eq!(msg, round);
    }

    #[test]
    fn wire_rejects_wrong_length() {
        assert!(EnrollmentProofMsg::from_wire(&[0u8; 63]).is_err());
        assert!(EnrollmentProofMsg::from_wire(&[0u8; 65]).is_err());
    }

    #[test]
    fn debug_never_leaks_secret() {
        let cap = CapabilitySecret::from_bytes([0xAB; 32]);
        let shown = format!("{cap:?}");
        assert!(shown.contains("redacted"));
        assert!(!shown.contains("abababab"));
    }

    // ---------------- property-based tests ----------------
    use proptest::prelude::*;

    prop_compose! {
        fn arb_secret()(bytes in any::<[u8; 32]>()) -> CapabilitySecret {
            CapabilitySecret::from_bytes(bytes)
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            failure_persistence: None,
            ..ProptestConfig::default()
        })]

        // A proof built from the roster's own capability, for the right
        // challenge and tree id, ALWAYS authorizes a fresh peer within expiry.
        #[test]
        fn honest_proof_always_authorizes(
            secret_bytes in any::<[u8; 32]>(),
            tree_id in "[a-zA-Z0-9-]{1,40}",
            challenge_bytes in any::<[u8; 32]>(),
            peer_bytes in any::<[u8; 32]>(),
            now in 0i64..1_000_000i64,
            slack in 1i64..1_000_000i64,
        ) {
            let cap = CapabilitySecret::from_bytes(secret_bytes);
            let mut r = ShareRoster::new(tree_id.clone(), cap.clone(), ExpiryTime(now + slack), 1);
            let ch = Challenge::from_bytes(challenge_bytes);
            let msg = EnrollmentProofMsg::build(&cap, &ch, &tree_id);
            let peer = PeerFingerprint::from_bytes(peer_bytes);
            let a = r.authorize(now, &ch, &msg.capability_id, &msg.proof, peer)
                .expect("honest proof must authorize");
            prop_assert!(a.newly_enrolled());
        }

        // Any capability DIFFERENT from the roster's is rejected (forgery),
        // regardless of challenge/tree/peer. `UnknownCapability` unless the
        // attacker somehow guessed the exact 256-bit secret (measure-zero).
        #[test]
        fn forged_capability_never_authorizes(
            real_bytes in any::<[u8; 32]>(),
            forged_bytes in any::<[u8; 32]>(),
            tree_id in "[a-zA-Z0-9-]{1,40}",
            challenge_bytes in any::<[u8; 32]>(),
            peer_bytes in any::<[u8; 32]>(),
            now in 0i64..1_000_000i64,
        ) {
            prop_assume!(real_bytes != forged_bytes);
            let real = CapabilitySecret::from_bytes(real_bytes);
            let forged = CapabilitySecret::from_bytes(forged_bytes);
            let mut r = ShareRoster::new(tree_id.clone(), real, ExpiryTime(now + 1000), 1);
            let ch = Challenge::from_bytes(challenge_bytes);
            let msg = EnrollmentProofMsg::build(&forged, &ch, &tree_id);
            let peer = PeerFingerprint::from_bytes(peer_bytes);
            let res = r.authorize(now, &ch, &msg.capability_id, &msg.proof, peer);
            prop_assert!(res.is_err(), "forged capability authorized: {res:?}");
            prop_assert_eq!(r.enrolled_count(), 0);
        }

        // A proof captured for challenge A never authorizes under a different
        // challenge B (replay across connections), for a fresh peer.
        #[test]
        fn proof_does_not_replay_across_challenges(
            secret_bytes in any::<[u8; 32]>(),
            tree_id in "[a-zA-Z0-9-]{1,40}",
            ch_a in any::<[u8; 32]>(),
            ch_b in any::<[u8; 32]>(),
            peer_bytes in any::<[u8; 32]>(),
        ) {
            prop_assume!(ch_a != ch_b);
            let cap = CapabilitySecret::from_bytes(secret_bytes);
            let mut r = ShareRoster::new(tree_id.clone(), cap.clone(), ExpiryTime(10_000), 1);
            let challenge_a = Challenge::from_bytes(ch_a);
            let captured = EnrollmentProofMsg::build(&cap, &challenge_a, &tree_id);
            // Acceptor issues challenge B on the new connection.
            let challenge_b = Challenge::from_bytes(ch_b);
            let peer = PeerFingerprint::from_bytes(peer_bytes);
            let res = r.authorize(
                100, &challenge_b, &captured.capability_id, &captured.proof, peer,
            );
            prop_assert_eq!(res, Err(AuthzReject::BadProof));
        }

        // No sequence of a valid holder + arbitrary strangers ever puts more
        // than `max_peers` distinct fingerprints on the roster.
        #[test]
        fn roster_never_exceeds_cap(
            secret_bytes in any::<[u8; 32]>(),
            tree_id in "[a-zA-Z0-9-]{1,40}",
            max in 1usize..4usize,
            peers in prop::collection::vec(any::<[u8; 32]>(), 0..12),
        ) {
            let cap = CapabilitySecret::from_bytes(secret_bytes);
            let mut r = ShareRoster::new(tree_id.clone(), cap.clone(), ExpiryTime(10_000), max);
            for pb in peers {
                let ch = Challenge::generate();
                let msg = EnrollmentProofMsg::build(&cap, &ch, &tree_id);
                let _ = r.authorize(100, &ch, &msg.capability_id, &msg.proof,
                    PeerFingerprint::from_bytes(pb));
                prop_assert!(r.enrolled_count() <= max);
            }
        }

        // Wire form round-trips for arbitrary bytes.
        #[test]
        fn wire_roundtrip_prop(cap_id in any::<[u8; 32]>(), proof in any::<[u8; 32]>()) {
            let msg = EnrollmentProofMsg {
                capability_id: CapabilityId::from_bytes(cap_id),
                proof: ProofTag::from_bytes(proof),
            };
            prop_assert_eq!(EnrollmentProofMsg::from_wire(&msg.to_wire()).unwrap(), msg);
        }
    }
}
