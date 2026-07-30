//! A2 mutual SAS pairing ceremony for self-devices (ADR 0028 A2).
//!
//! # Why a SAS, and why mandatory for self-devices
//!
//! A third-party share uses TOFU (A1): the capability reaches the recipient
//! over a trusted out-of-band channel and whoever proves it first is enrolled.
//! For the owner's *own* devices the ruling is stricter: a **mandatory Short
//! Authentication String** the user compares on both screens, so a
//! man-in-the-middle who relays the QUIC handshake cannot silently interpose.
//! Only after a confirmed SAS match does the owner sign the new device into the
//! B1 roster.
//!
//! # Construction (documented precisely)
//!
//! Commit-then-reveal over both devices' QUIC node public keys, bound to the
//! share's capability id:
//!
//! 1. Each side draws a fresh 32-byte `nonce` and publishes `commitment =
//!    blake3(COMMIT_DOMAIN ‖ capability_id ‖ role ‖ pubkey ‖ nonce)`.
//! 2. After exchanging commitments, each side reveals `(pubkey, nonce)`.
//! 3. Each side recomputes the peer's commitment from the peer's reveal and its
//!    (opposite) role; a mismatch is a **loud abort** — a MITM cannot open a
//!    commitment to a different nonce (blake3 binding).
//! 4. Both sides derive the SAS from a role-ordered transcript
//!    `blake3(SAS_DOMAIN ‖ capability_id ‖ init.pubkey ‖ init.nonce ‖
//!    resp.pubkey ‖ resp.nonce)` and show its **6 digits + 5 emoji**. The user
//!    compares; equal ⇒ confirm, differ ⇒ abort.
//!
//! # Why the classic attacks are unrepresentable or loud
//!
//! - **Forgery / relay tamper** — a MITM must commit before learning the honest
//!   nonce and cannot make two transcripts collide (blake3), so the two SAS
//!   differ and the user aborts. Opening a commitment to a swapped nonce is
//!   caught at step 3 ([`PairingError::CommitmentMismatch`]).
//! - **Reflection / self-pairing** — a peer echoing our own pubkey back is
//!   rejected ([`PairingError::SelfPairing`]).
//! - **Replay** — a captured reveal from an old session is consistent with its
//!   own commitment but carries a stale nonce, so the derived SAS differs from
//!   the honest side's ⇒ abort. Fresh nonces per session.
//! - **Wrong share** — the capability id is folded into both the commitment and
//!   the transcript, so a SAS from share A never matches share B.
//!
//! Equality is compared over a `blake3::Hash` (documented constant-time
//! `PartialEq`), never raw bytes.

use blake3::Hash;

const COMMIT_DOMAIN: &[u8] = b"holon.sas.commit.v1";
const SAS_DOMAIN: &[u8] = b"holon.sas.transcript.v1";

/// 64 visually-distinct emoji — 6 bits each. Fixed forever: changing the table
/// changes every SAS.
const SAS_EMOJI: [&str; 64] = [
    "🐶", "🐱", "🐭", "🐹", "🐰", "🦊", "🐻", "🐼", "🐨", "🐯", "🦁", "🐮", "🐷", "🐸", "🐵", "🐔",
    "🐧", "🐦", "🐤", "🦆", "🦉", "🐴", "🦄", "🐝", "🐛", "🦋", "🐌", "🐞", "🐢", "🐍", "🐙", "🦀",
    "🐬", "🐳", "🐊", "🐘", "🐫", "🦒", "🦓", "🦏", "🌵", "🌲", "🍄", "🌸", "🌻", "🍎", "🍋", "🍉",
    "🍇", "🍓", "🌶", "🌽", "⭐", "🌙", "☀", "🔥", "💧", "❄", "🎈", "🎁", "🔔", "🎸", "🚀", "⚓",
];

/// Which end of the pairing this device is. Folded into the commitment so a
/// reflected message is caught, and it fixes the transcript order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SasRole {
    Initiator,
    Responder,
}

impl SasRole {
    fn tag(self) -> u8 {
        match self {
            SasRole::Initiator => 0,
            SasRole::Responder => 1,
        }
    }

    fn opposite(self) -> SasRole {
        match self {
            SasRole::Initiator => SasRole::Responder,
            SasRole::Responder => SasRole::Initiator,
        }
    }
}

/// A device's commitment to its (pubkey, nonce) contribution. Public.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SasCommitment(Hash);

impl SasCommitment {
    pub fn to_bytes(self) -> [u8; 32] {
        *self.0.as_bytes()
    }
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        SasCommitment(Hash::from_bytes(bytes))
    }
}

/// A device's opened contribution, exchanged after commitments. Public.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SasReveal {
    pub pubkey: [u8; 32],
    pub nonce: [u8; 32],
}

/// Loud pairing failures. Every variant means: refuse the pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingError {
    /// The peer's reveal does not open the commitment it earlier sent — a MITM
    /// or a corrupted message.
    CommitmentMismatch,
    /// The peer presented our own node key (reflection / pairing with self).
    SelfPairing,
}

impl std::fmt::Display for PairingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PairingError::CommitmentMismatch => f.write_str(
                "SAS pairing aborted: peer reveal did not open its commitment \
                 (possible man-in-the-middle)",
            ),
            PairingError::SelfPairing => f.write_str(
                "SAS pairing aborted: peer presented this device's own node key \
                 (reflection / self-pairing)",
            ),
        }
    }
}

impl std::error::Error for PairingError {}

fn commit(role: SasRole, capability_id: &[u8; 32], pubkey: &[u8; 32], nonce: &[u8; 32]) -> Hash {
    let mut h = blake3::Hasher::new();
    h.update(COMMIT_DOMAIN);
    h.update(capability_id);
    h.update(&[role.tag()]);
    h.update(pubkey);
    h.update(nonce);
    h.finalize()
}

/// A pairing session in its initial phase: local contribution drawn, commitment
/// ready to send. Advance by feeding the peer's commitment.
#[derive(Debug, Clone)]
pub struct PairingSession {
    role: SasRole,
    capability_id: [u8; 32],
    self_pubkey: [u8; 32],
    self_nonce: [u8; 32],
}

impl PairingSession {
    /// Begin a pairing as `role` for the share named by `capability_id`, using
    /// this device's QUIC node key `self_pubkey`. Draws a fresh nonce.
    pub fn begin(role: SasRole, capability_id: [u8; 32], self_pubkey: [u8; 32]) -> Self {
        let mut self_nonce = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rng(), &mut self_nonce);
        Self {
            role,
            capability_id,
            self_pubkey,
            self_nonce,
        }
    }

    /// Deterministic constructor for tests / replay — nonce supplied.
    pub fn begin_with_nonce(
        role: SasRole,
        capability_id: [u8; 32],
        self_pubkey: [u8; 32],
        self_nonce: [u8; 32],
    ) -> Self {
        Self {
            role,
            capability_id,
            self_pubkey,
            self_nonce,
        }
    }

    /// This device's commitment — send it to the peer first.
    pub fn commitment(&self) -> SasCommitment {
        SasCommitment(commit(
            self.role,
            &self.capability_id,
            &self.self_pubkey,
            &self.self_nonce,
        ))
    }

    /// Having received the peer's commitment, move to the reveal phase and
    /// produce this device's reveal to send.
    pub fn reveal(self, peer_commitment: SasCommitment) -> (AwaitingReveal, SasReveal) {
        let reveal = SasReveal {
            pubkey: self.self_pubkey,
            nonce: self.self_nonce,
        };
        let next = AwaitingReveal {
            role: self.role,
            capability_id: self.capability_id,
            self_pubkey: self.self_pubkey,
            self_nonce: self.self_nonce,
            peer_commitment,
        };
        (next, reveal)
    }
}

/// The reveal phase: commitments exchanged, waiting for the peer's reveal to
/// verify and derive the SAS.
#[derive(Debug, Clone)]
pub struct AwaitingReveal {
    role: SasRole,
    capability_id: [u8; 32],
    self_pubkey: [u8; 32],
    self_nonce: [u8; 32],
    peer_commitment: SasCommitment,
}

impl AwaitingReveal {
    /// Verify the peer's reveal against its commitment and derive the SAS.
    /// Loud on any inconsistency — a returned `Ok` means the transcript is
    /// bound; the *human* SAS comparison still gates final confirmation.
    pub fn derive_sas(self, peer_reveal: SasReveal) -> Result<Sas, PairingError> {
        if peer_reveal.pubkey == self.self_pubkey {
            return Err(PairingError::SelfPairing);
        }
        // The peer's reveal must open the commitment it sent, under the peer's
        // (opposite) role.
        let recomputed = commit(
            self.role.opposite(),
            &self.capability_id,
            &peer_reveal.pubkey,
            &peer_reveal.nonce,
        );
        if SasCommitment(recomputed) != self.peer_commitment {
            return Err(PairingError::CommitmentMismatch);
        }

        let self_reveal = SasReveal {
            pubkey: self.self_pubkey,
            nonce: self.self_nonce,
        };
        let (init, resp) = match self.role {
            SasRole::Initiator => (&self_reveal, &peer_reveal),
            SasRole::Responder => (&peer_reveal, &self_reveal),
        };
        let mut h = blake3::Hasher::new();
        h.update(SAS_DOMAIN);
        h.update(&self.capability_id);
        h.update(&init.pubkey);
        h.update(&init.nonce);
        h.update(&resp.pubkey);
        h.update(&resp.nonce);
        Ok(Sas::from_digest(h.finalize()))
    }
}

/// The derived short authentication string. Equality is constant-time (via
/// `blake3::Hash`). The user compares the [`Self::digits`] / [`Self::emoji`]
/// forms across both devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sas {
    digest: Hash,
}

impl Sas {
    fn from_digest(digest: Hash) -> Self {
        Sas { digest }
    }

    /// 6 decimal digits, zero-padded.
    pub fn digits(&self) -> String {
        let b = self.digest.as_bytes();
        let n = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) % 1_000_000;
        format!("{n:06}")
    }

    /// 5 emoji drawn 6 bits at a time from the next 30 bits of the digest.
    pub fn emoji(&self) -> Vec<&'static str> {
        let b = self.digest.as_bytes();
        let bits = u32::from_be_bytes([b[4], b[5], b[6], b[7]]);
        (0..5)
            .map(|i| {
                let idx = ((bits >> (26 - i * 6)) & 0x3f) as usize;
                SAS_EMOJI[idx]
            })
            .collect()
    }

    /// Constant-time comparison of two derived SAS. The two devices call this
    /// (or the user compares the rendered forms) to confirm the pairing.
    pub fn matches(&self, other: &Sas) -> bool {
        // `blake3::Hash: PartialEq` is constant-time.
        self.digest == other.digest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(seed: u8) -> [u8; 32] {
        [seed; 32]
    }

    /// Run the honest ceremony end to end; both sides must derive an equal SAS.
    fn honest_run(
        cap: [u8; 32],
        a_pk: [u8; 32],
        b_pk: [u8; 32],
        a_nonce: [u8; 32],
        b_nonce: [u8; 32],
    ) -> (Result<Sas, PairingError>, Result<Sas, PairingError>) {
        let a = PairingSession::begin_with_nonce(SasRole::Initiator, cap, a_pk, a_nonce);
        let b = PairingSession::begin_with_nonce(SasRole::Responder, cap, b_pk, b_nonce);
        let a_commit = a.commitment();
        let b_commit = b.commitment();
        let (a_wait, a_reveal) = a.reveal(b_commit);
        let (b_wait, b_reveal) = b.reveal(a_commit);
        (a_wait.derive_sas(b_reveal), b_wait.derive_sas(a_reveal))
    }

    #[test]
    fn honest_pairing_agrees() {
        let (sa, sb) = honest_run(pk(9), pk(1), pk(2), pk(10), pk(20));
        let (sa, sb) = (sa.unwrap(), sb.unwrap());
        assert!(sa.matches(&sb));
        assert_eq!(sa.digits(), sb.digits());
        assert_eq!(sa.emoji(), sb.emoji());
        assert_eq!(sa.digits().len(), 6);
        assert_eq!(sa.emoji().len(), 5);
    }

    #[test]
    fn tampered_nonce_is_caught_as_commitment_mismatch() {
        // MITM lets commitments through but swaps B's revealed nonce.
        let cap = pk(9);
        let a = PairingSession::begin_with_nonce(SasRole::Initiator, cap, pk(1), pk(10));
        let b = PairingSession::begin_with_nonce(SasRole::Responder, cap, pk(2), pk(20));
        let a_commit = a.commitment();
        let b_commit = b.commitment();
        let (a_wait, _a_reveal) = a.reveal(b_commit);
        let (_b_wait, mut b_reveal) = b.reveal(a_commit);
        b_reveal.nonce = pk(99); // does not open B's commitment
        assert_eq!(
            a_wait.derive_sas(b_reveal).unwrap_err(),
            PairingError::CommitmentMismatch
        );
    }

    #[test]
    fn reflection_is_rejected() {
        let cap = pk(9);
        let a = PairingSession::begin_with_nonce(SasRole::Initiator, cap, pk(1), pk(10));
        let b_commit = SasCommitment::from_bytes([0u8; 32]);
        let (a_wait, _) = a.reveal(b_commit);
        // Peer reflects A's own pubkey back.
        let reflected = SasReveal {
            pubkey: pk(1),
            nonce: pk(77),
        };
        assert_eq!(
            a_wait.derive_sas(reflected).unwrap_err(),
            PairingError::SelfPairing
        );
    }

    #[test]
    fn different_capability_yields_different_sas() {
        let (a1, _) = honest_run(pk(1), pk(1), pk(2), pk(10), pk(20));
        let (a2, _) = honest_run(pk(2), pk(1), pk(2), pk(10), pk(20));
        assert!(!a1.unwrap().matches(&a2.unwrap()));
    }

    #[test]
    fn replayed_stale_reveal_diverges_from_honest_sas() {
        // A pairs honestly with B (nonce 20). Later a replayed B-reveal from an
        // old session (nonce 21) is fed with a matching stale commitment: it
        // opens fine but the SAS differs from the live one, so the user aborts.
        let cap = pk(9);
        let (live_a, _) = honest_run(cap, pk(1), pk(2), pk(10), pk(20));
        let stale_b = PairingSession::begin_with_nonce(SasRole::Responder, cap, pk(2), pk(21));
        let a = PairingSession::begin_with_nonce(SasRole::Initiator, cap, pk(1), pk(10));
        let (a_wait, _) = a.reveal(stale_b.commitment());
        let (_sb, stale_reveal) = stale_b.reveal(SasCommitment::from_bytes([0u8; 32]));
        let replayed = a_wait.derive_sas(stale_reveal).unwrap();
        assert!(!replayed.matches(&live_a.unwrap()));
    }
}

#[cfg(test)]
mod pbt {
    use proptest::prelude::*;

    use super::*;

    fn arb32() -> impl Strategy<Value = [u8; 32]> {
        any::<[u8; 32]>()
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            failure_persistence: None,
            ..ProptestConfig::default()
        })]

        /// Honest ceremony over arbitrary keys/nonces always agrees, for any
        /// distinct node keys.
        #[test]
        fn honest_always_agrees(
            cap in arb32(),
            a_pk in arb32(),
            b_pk in arb32(),
            a_nonce in arb32(),
            b_nonce in arb32(),
        ) {
            prop_assume!(a_pk != b_pk);
            let a = PairingSession::begin_with_nonce(SasRole::Initiator, cap, a_pk, a_nonce);
            let b = PairingSession::begin_with_nonce(SasRole::Responder, cap, b_pk, b_nonce);
            let (a_wait, a_reveal) = a.clone().reveal(b.commitment());
            let (b_wait, b_reveal) = b.reveal(a.commitment());
            let sa = a_wait.derive_sas(b_reveal).unwrap();
            let sb = b_wait.derive_sas(a_reveal).unwrap();
            prop_assert!(sa.matches(&sb));
            prop_assert_eq!(sa.digits(), sb.digits());
        }

        /// Any tamper with the revealed nonce is caught loudly — never a silent
        /// wrong-SAS-that-still-matches.
        #[test]
        fn nonce_tamper_never_silently_matches(
            cap in arb32(),
            a_pk in arb32(),
            b_pk in arb32(),
            a_nonce in arb32(),
            b_nonce in arb32(),
            evil_nonce in arb32(),
        ) {
            prop_assume!(a_pk != b_pk);
            prop_assume!(b_nonce != evil_nonce);
            let a = PairingSession::begin_with_nonce(SasRole::Initiator, cap, a_pk, a_nonce);
            let b = PairingSession::begin_with_nonce(SasRole::Responder, cap, b_pk, b_nonce);
            let (a_wait, _) = a.reveal(b.commitment());
            let evil = SasReveal { pubkey: b_pk, nonce: evil_nonce };
            prop_assert_eq!(
                a_wait.derive_sas(evil).unwrap_err(),
                PairingError::CommitmentMismatch
            );
        }

        /// Distinct shares never collide their SAS (capability binding).
        #[test]
        fn distinct_shares_distinct_sas(
            cap_a in arb32(),
            cap_b in arb32(),
            a_pk in arb32(),
            b_pk in arb32(),
            a_nonce in arb32(),
            b_nonce in arb32(),
        ) {
            prop_assume!(cap_a != cap_b);
            prop_assume!(a_pk != b_pk);
            let run = |cap: [u8; 32]| {
                let a = PairingSession::begin_with_nonce(SasRole::Initiator, cap, a_pk, a_nonce);
                let b = PairingSession::begin_with_nonce(SasRole::Responder, cap, b_pk, b_nonce);
                let (aw, _) = a.clone().reveal(b.commitment());
                let (_bw, br) = b.reveal(a.commitment());
                aw.derive_sas(br).unwrap()
            };
            prop_assert!(!run(cap_a).matches(&run(cap_b)));
        }
    }
}
