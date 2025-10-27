//! Versioned ticket for sharing a Loro subtree over Iroh.
//!
//! A ticket is a bearer capability: anyone holding it can read and write the
//! shared subtree until the share is dropped. See docs/SUBTREE_SHARING.md.
//!
//! Wire format: JSON, wrapped in base64 (URL-safe, no padding) for robust
//! copy-paste across chat apps. The `v` field lets the schema evolve.

use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use iroh::EndpointAddr;
use serde::{Deserialize, Serialize};

pub const TICKET_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ticket {
    pub v: u32,
    pub shared_tree_id: String,
    pub addr: EndpointAddr,
    pub alpn: String,
}

impl Ticket {
    pub fn new(shared_tree_id: String, addr: EndpointAddr, alpn: String) -> Self {
        Self {
            v: TICKET_VERSION,
            shared_tree_id,
            addr,
            alpn,
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
    use super::*;
    use iroh::SecretKey;

    fn sample_addr() -> EndpointAddr {
        let sk = SecretKey::generate(&mut rand::thread_rng());
        EndpointAddr::new(sk.public())
    }

    #[test]
    fn round_trip() {
        let t = Ticket::new(
            "share-abc".into(),
            sample_addr(),
            "loro-sync/share-abc".into(),
        );
        let encoded = t.encode().unwrap();
        let decoded = Ticket::decode(&encoded).unwrap();
        assert_eq!(t, decoded);
    }

    #[test]
    fn bad_base64_errors() {
        let err = Ticket::decode("!!!not-base64!!!").unwrap_err();
        assert!(format!("{err:#}").contains("base64"));
    }

    #[test]
    fn wrong_version_errors() {
        let mut t = Ticket::new("x".into(), sample_addr(), "loro-sync/x".into());
        t.v = 99;
        let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&t).unwrap());
        let err = Ticket::decode(&encoded).unwrap_err();
        assert!(format!("{err:#}").contains("version"));
    }

    #[test]
    fn trims_whitespace() {
        let t = Ticket::new("y".into(), sample_addr(), "loro-sync/y".into());
        let encoded = format!("  \n{}  \n", t.encode().unwrap());
        assert_eq!(Ticket::decode(&encoded).unwrap(), t);
    }
}
