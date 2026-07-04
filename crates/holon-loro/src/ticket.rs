//! Versioned ticket for sharing a Loro subtree over Iroh.
//!
//! A ticket carries a **capability secret** ([`CapabilitySecret`]): holding it
//! lets a peer *enroll* into the share (prove possession over a fresh
//! challenge — see [`crate::share_enrollment`]). The `shared_tree_id` is NOT
//! the access secret; it leaks to disk/logs/wire. The capability is the secret,
//! so it must NOT be logged — hence the redacted `Debug` on
//! `CapabilitySecret`. See docs/Reference/SUBTREE_SHARING.md.
//!
//! NOTE (enforcement status): v2 tickets *carry* the capability + expiry, but
//! the acceptor-side gate that verifies them
//! (`share_enrollment::acceptor_enroll` wired into the advertiser) lands with
//! the enrollment-ceremony ruling (ADR 0028 §H5/C1'). Until then the capability
//! is forward-compat plumbing, not yet an enforced boundary — do not treat a v2
//! ticket as authenticated.
//!
//! Wire format: JSON, wrapped in base64 (URL-safe, no padding) for robust
//! copy-paste across chat apps. The `v` field lets the schema evolve.

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use iroh::EndpointAddr;
use serde::Deserialize;
use serde::Serialize;

use crate::share_enrollment::CapabilitySecret;
use crate::share_enrollment::ExpiryTime;

/// v2 adds `capability` + `expires_at`. v1 tickets (no capability) are rejected
/// loudly on decode — a v1 ticket cannot be authorized under the new model.
pub const TICKET_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ticket {
    pub v: u32,
    pub shared_tree_id: String,
    pub addr: EndpointAddr,
    pub alpn: String,
    /// The share access secret the holder proves possession of at enrollment.
    pub capability: CapabilitySecret,
    /// Enrollment deadline, Unix seconds. After this, no *new* peer may enroll.
    pub expires_at: ExpiryTime,
}

impl Ticket {
    pub fn new(
        shared_tree_id: String,
        addr: EndpointAddr,
        alpn: String,
        capability: CapabilitySecret,
        expires_at: ExpiryTime,
    ) -> Self {
        Self {
            v: TICKET_VERSION,
            shared_tree_id,
            addr,
            alpn,
            capability,
            expires_at,
        }
    }

    pub fn encode(&self) -> Result<String> {
        let json = serde_json::to_vec(self).context("serialize ticket")?;
        Ok(URL_SAFE_NO_PAD.encode(json))
    }

    pub fn decode(s: &str) -> Result<Self> {
        let bytes = URL_SAFE_NO_PAD
            .decode(s.trim())
            .context("ticket is not valid base64 (url-safe, no padding)")?;
        let ticket: Self =
            serde_json::from_slice(&bytes).context("ticket JSON did not match schema")?;
        if ticket.v != TICKET_VERSION {
            bail!(
                "unsupported ticket version {} (this build supports v{})",
                ticket.v,
                TICKET_VERSION
            );
        }
        Ok(ticket)
    }
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;

    use super::*;

    fn sample_addr() -> EndpointAddr {
        let sk = SecretKey::generate(&mut rand::rng());
        EndpointAddr::new(sk.public())
    }

    fn sample_ticket(id: &str) -> Ticket {
        Ticket::new(
            id.into(),
            sample_addr(),
            format!("loro-sync/{id}"),
            CapabilitySecret::generate(),
            ExpiryTime(1_000_000),
        )
    }

    #[test]
    fn round_trip() {
        let t = sample_ticket("share-abc");
        let encoded = t.encode().unwrap();
        let decoded = Ticket::decode(&encoded).unwrap();
        assert_eq!(t, decoded);
    }

    #[test]
    fn capability_survives_round_trip() {
        let t = sample_ticket("share-cap");
        let decoded = Ticket::decode(&t.encode().unwrap()).unwrap();
        assert_eq!(
            t.capability.capability_id(),
            decoded.capability.capability_id()
        );
    }

    #[test]
    fn encoded_ticket_does_not_expose_secret_in_debug() {
        // The secret is base64 in the ticket wire form (by design), but its
        // Debug must never print the raw bytes.
        let t = sample_ticket("share-dbg");
        assert!(format!("{:?}", t.capability).contains("redacted"));
    }

    #[test]
    fn bad_base64_errors() {
        let err = Ticket::decode("!!!not-base64!!!").unwrap_err();
        assert!(format!("{err:#}").contains("base64"));
    }

    #[test]
    fn wrong_version_errors() {
        let mut t = sample_ticket("x");
        t.v = 99;
        let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&t).unwrap());
        let err = Ticket::decode(&encoded).unwrap_err();
        assert!(format!("{err:#}").contains("version"));
    }

    #[test]
    fn v1_ticket_rejected() {
        // A legacy v1 ticket has no capability field and v=1; decode must fail
        // loudly rather than silently accept an un-authorizable share.
        let legacy = serde_json::json!({
            "v": 1,
            "shared_tree_id": "old",
            "addr": sample_addr(),
            "alpn": "loro-sync/old",
        });
        let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&legacy).unwrap());
        assert!(Ticket::decode(&encoded).is_err());
    }

    #[test]
    fn trims_whitespace() {
        let t = sample_ticket("y");
        let encoded = format!("  \n{}  \n", t.encode().unwrap());
        assert_eq!(Ticket::decode(&encoded).unwrap(), t);
    }
}
