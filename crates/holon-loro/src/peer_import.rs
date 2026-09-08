//! The one gate a remote peer's CRDT delta passes to reach a replicated
//! document over the iroh transport (D86.a).
//!
//! ## What the type buys
//! [`import_peer_delta`] is the only function in this crate that writes peer
//! bytes into a doc, and it cannot be called without an [`AdmittedPeer`].
//! `AdmittedPeer` has no public field and no `Default`: it exists only where an
//! admission decision was taken, so "an import with no decision behind it" is
//! not a state a caller can reach — the way `AuthorizedPeer` already works for
//! enrollment, one layer up.
//!
//! ## Which capability each direction exercises
//! Applying a peer's ops into THIS device's replica is that peer writing here,
//! so an import needs [`Capability::Write`]; handing the peer our delta is that
//! peer reading here, so an export needs [`Capability::Read`]
//! ([`authorize_peer_read`]). That is the same rule
//! `holon_sharing::acceptor::required_capability` states for the relay leg, and
//! the set travels from the admission as a [`Capabilities`] value — never
//! re-derived from a string at the point of use.
//!
//! ## What an admission does NOT prove
//! Only that the peer is a member of this share with these capabilities. The
//! identity behind it is the QUIC/TLS-authenticated iroh node key, so this is
//! exactly as strong as the transport's peer authentication and no stronger. An
//! un-gated share (see [`crate::iroh_advertiser::ShareAdmission::Ungated`])
//! admits whoever reaches the endpoint.

use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use holon_api::sharing::Capabilities;
use holon_api::sharing::Capability;
use loro::LoroDoc;

use crate::loro_document::LoroDocument;
use crate::loro_document::SYNC_IMPORT_ORIGIN;
use crate::share_enrollment::AuthorizedPeer;
use crate::share_enrollment::PeerFingerprint;

/// Proof that a specific remote peer is admitted to a specific container, and
/// with which capabilities. Constructible only through the named constructors
/// below, each of which states the basis of its decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmittedPeer {
    peer: PeerFingerprint,
    container: String,
    capabilities: Capabilities,
}

impl AdmittedPeer {
    /// Acceptor side: the peer proved the share capability (or presented an
    /// owner-signed device entry) against this share's roster, which is what
    /// `capabilities` rests on.
    pub fn enrolled(
        container: impl Into<String>,
        authorized: &AuthorizedPeer,
        capabilities: Capabilities,
    ) -> Self {
        Self {
            peer: authorized.peer(),
            container: container.into(),
            capabilities,
        }
    }

    /// Initiator side: THIS device chose the peer, dialed it for `container`,
    /// and (on the enrolled dial) proved its own membership to it. There is no
    /// roster on this side, so `capabilities` is what this device grants the
    /// peer it dialed — the caller must be able to say why.
    pub fn dialed(
        container: impl Into<String>,
        peer: PeerFingerprint,
        capabilities: Capabilities,
    ) -> Self {
        Self {
            peer,
            container: container.into(),
            capabilities,
        }
    }

    /// No proof was required: the share is advertised un-gated, so the peer is
    /// whoever reached the endpoint. Named apart from the two decisions above
    /// because it is not one — it records that the caller chose to admit a
    /// stranger, and with what. See
    /// [`crate::iroh_advertiser::ShareAdmission::Ungated`].
    pub fn ungated(
        container: impl Into<String>,
        peer: PeerFingerprint,
        capabilities: Capabilities,
    ) -> Self {
        Self {
            peer,
            container: container.into(),
            capabilities,
        }
    }

    pub fn peer(&self) -> PeerFingerprint {
        self.peer
    }

    pub fn container(&self) -> &str {
        &self.container
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn refuse(&self, missing: Capability, attempted: String) -> PeerAccessRefused {
        PeerAccessRefused {
            peer: self.peer,
            container: self.container.clone(),
            missing,
            held: self.capabilities.clone(),
            attempted,
        }
    }
}

/// An access the admission does not authorize. A refusal is a loud `Err`, never
/// a silent drop: it names the peer, the container, the missing capability and
/// what was attempted, so a misissued grant is diagnosable from one log line.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error(
    "peer {peer:?} is admitted to container `{container}` with capabilities {held}, which do not \
     include `{missing}` — {attempted}; refusing"
)]
pub struct PeerAccessRefused {
    pub peer: PeerFingerprint,
    pub container: String,
    pub missing: Capability,
    pub held: Capabilities,
    /// What the peer tried to do, in words, so the message reads as a sentence.
    pub attempted: String,
}

/// Apply an admitted peer's delta into `doc`.
///
/// Two things happen here that a bare `LoroDoc::import` does not do: the
/// admission's capabilities are enforced, and the write goes through the
/// document's own boundary lock tagged [`SYNC_IMPORT_ORIGIN`] — so the import
/// can neither land inside a local batch's interior nor be mistaken by a
/// subscriber for this device's own write.
///
/// `doc` is the raw `Arc<LoroDoc>` the transport carries. Re-wrapping it
/// resolves to the SAME lock (the registry is keyed by the `Arc`'s identity —
/// see [`crate::doc_lock`]), so the seal survives the transport's escape.
pub fn import_peer_delta(doc: &Arc<LoroDoc>, admitted: &AdmittedPeer, delta: &[u8]) -> Result<()> {
    if !admitted.capabilities.contains(Capability::Write) {
        return Err(admitted
            .refuse(
                Capability::Write,
                format!(
                    "importing its {}-byte delta would be that peer writing into this replica",
                    delta.len()
                ),
            )
            .into());
    }
    LoroDocument::from_existing(doc.clone(), admitted.container.clone())
        .apply_update_with_origin(SYNC_IMPORT_ORIGIN, delta)
        .with_context(|| {
            format!(
                "importing an ADMITTED {}-byte delta from peer {:?} into container `{}` — \
                 admission succeeded, so a failure here is corruption, not authorization",
                delta.len(),
                admitted.peer,
                admitted.container
            )
        })
}

/// Proof that an admitted peer holds [`Capability::Read`] for its container.
/// Minted only by [`authorize_peer_read`], so a value of this type cannot exist
/// for a peer the admission refused.
///
/// It carries the container name, which is why the sync leg and the
/// remember-this-peer callback take it instead of a bare `String`: neither can
/// name a container without first having passed the read gate for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerReadAccess {
    admitted: AdmittedPeer,
}

impl PeerReadAccess {
    /// The admission the read was authorized against — what the import
    /// direction is then checked against.
    pub fn admitted(&self) -> &AdmittedPeer {
        &self.admitted
    }

    pub fn container(&self) -> &str {
        self.admitted.container()
    }

    pub fn peer(&self) -> PeerFingerprint {
        self.admitted.peer()
    }
}

/// Gate the OTHER direction: handing this peer our state is that peer reading
/// here. Called before the delta is exported, so a peer whose admission confers
/// no `Read` never sees container bytes — and, because the witness is also what
/// records a peer as dialable, never becomes one this device dials back.
pub fn authorize_peer_read(admitted: &AdmittedPeer) -> Result<PeerReadAccess> {
    if admitted.capabilities.contains(Capability::Read) {
        return Ok(PeerReadAccess {
            admitted: admitted.clone(),
        });
    }
    Err(admitted
        .refuse(
            Capability::Read,
            "sending it this container's delta would be that peer reading here".to_string(),
        )
        .into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loro_backend::TREE_NAME;

    fn peer() -> PeerFingerprint {
        PeerFingerprint::from_bytes([9u8; 32])
    }

    /// A snapshot that creates one tree node — the delta a peer would push.
    fn source_delta() -> Vec<u8> {
        let doc = LoroDoc::new();
        doc.set_peer_id(1).unwrap();
        let tree = doc.get_tree(TREE_NAME);
        tree.enable_fractional_index(0);
        tree.create(None).unwrap();
        doc.commit();
        doc.export(loro::ExportMode::Snapshot).unwrap()
    }

    fn target() -> Arc<LoroDoc> {
        let doc = LoroDoc::new();
        doc.set_peer_id(2).unwrap();
        Arc::new(doc)
    }

    #[test]
    fn a_read_only_peers_delta_is_refused_naming_the_missing_capability() {
        let doc = target();
        let admitted = AdmittedPeer::dialed("holon_tree", peer(), Capabilities::read_only());

        let err = import_peer_delta(&doc, &admitted, &source_delta())
            .expect_err("a read-only peer's delta must not enter the replica");
        let refusal = err
            .downcast_ref::<PeerAccessRefused>()
            .expect("the refusal is typed, so a caller can tell it from an I/O failure");
        assert_eq!(refusal.missing, Capability::Write);
        assert_eq!(refusal.held, Capabilities::read_only());
        assert_eq!(refusal.container, "holon_tree");

        assert_eq!(
            doc.get_tree(TREE_NAME).nodes().len(),
            0,
            "the refused delta must leave the replica untouched"
        );
    }

    #[test]
    fn a_read_write_peers_delta_lands_tagged_sync_import() {
        let doc = target();
        let admitted = AdmittedPeer::dialed("holon_tree", peer(), Capabilities::read_write());

        let origins = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let sink = origins.clone();
        let _sub = doc.subscribe_root(Arc::new(move |event| {
            sink.lock()
                .expect("origin sink")
                .push(event.origin.to_string());
        }));

        import_peer_delta(&doc, &admitted, &source_delta()).expect("an admitted write imports");

        assert_eq!(doc.get_tree(TREE_NAME).nodes().len(), 1);
        assert_eq!(
            origins.lock().expect("origin sink").as_slice(),
            [SYNC_IMPORT_ORIGIN.to_string()],
            "a peer import must be distinguishable from this device's own write"
        );
    }

    /// An admission that confers nothing at all is still a decision, and it
    /// refuses in BOTH directions — there is no "empty means allow" path.
    #[test]
    fn an_admission_conferring_nothing_refuses_both_directions() {
        let doc = target();
        let admitted = AdmittedPeer::dialed("holon_tree", peer(), Capabilities::default());
        assert!(import_peer_delta(&doc, &admitted, &source_delta()).is_err());
        assert!(authorize_peer_read(&admitted).is_err());
    }

    #[test]
    fn a_write_only_admission_may_not_read() {
        let admitted =
            AdmittedPeer::dialed("holon_tree", peer(), Capabilities::of([Capability::Write]));
        let err = authorize_peer_read(&admitted).expect_err("no Read, no export");
        assert_eq!(
            err.downcast_ref::<PeerAccessRefused>()
                .expect("typed refusal")
                .missing,
            Capability::Read
        );
    }
}
