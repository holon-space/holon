//! Owner-private alias ledger — the re-encode baseline (ADR 0028 H4 / A4).
//!
//! Narrowing a share rotates its container: the owner **re-encodes** the
//! subtree into a fresh container whose blocks all get **fresh public ids**,
//! and signs a **succession pointer** (`old → new`; `old` frozen/tombstoned).
//! This is the baseline that needs no loro internals (the shallow-fork upgrade
//! is W2, out of scope, behind this same succession interface).
//!
//! Recipients — including an overlapping-audience peer — see only `new` with
//! fresh ids: they get **no correlation handle** back to `old` (A4: "same id
//! vanished here, appeared there" must be impossible). The owner, however,
//! keeps an **owner-private alias ledger** (`old-id → new-id` chains) so its
//! own history stitches: owner backlinks are rewritten through the SQL
//! `block_links` junction ([`owner_backlink_rewrite_sql`]). Recipients'
//! backlinks to crossed-out blocks dangle *correctly* — `resolved_id` goes NULL
//! and the existing renderer shows a loud unresolved ref, never a silent drop
//! (fail-loud).
//!
//! ## OQ3 — non-replicated storage
//! The ledger is a **dedicated `LoroDoc` excluded from the C1 replication
//! set**. Since the C1 registry doesn't exist yet (Inc 3), the exclusion is
//! encoded as the [`NonReplicated`] typed marker: the registry (Inc 3) will
//! honor it by never enumerating a `NonReplicated` doc in its replicate-all
//! iteration. Wrapping the doc in the marker makes "this never leaves the
//! owner's device" a property of the TYPE, not a convention.

use loro::LoroDoc;

use crate::types::BlockId;
use crate::types::ContainerId;
use crate::types::OwnerSig;
use crate::types::SigningAuthority;
use crate::types::StablePeerId;

const ALIAS_MAP: &str = "block_aliases";
const SUCCESSION_LIST: &str = "container_successions";

/// A typed marker: the wrapped handle is **owner-private** and MUST be excluded
/// from the C1 replication set (OQ3). The container registry (Inc 3, pending)
/// honors this by never enumerating a `NonReplicated` doc in its replicate-all
/// iteration.
///
/// CONTRACT: there is deliberately NO API returning the inner handle to a
/// *replication* caller — only [`owner_local`](Self::owner_local) /
/// [`owner_local_mut`](Self::owner_local_mut), whose names assert the access is
/// owner-scoped. The registry sees only the marker.
#[derive(Debug)]
pub struct NonReplicated<T>(T);

impl<T> NonReplicated<T> {
    pub fn new(inner: T) -> Self {
        Self(inner)
    }

    /// Owner-local read access. Never call this from a replication code path.
    pub fn owner_local(&self) -> &T {
        &self.0
    }

    /// Owner-local write access. Never call this from a replication code path.
    pub fn owner_local_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

/// An owner-signed pointer recording that container `old` was rotated
/// (re-encoded) into a fresh container `new`; `old` is FROZEN. Owner-private:
/// recipients never see this.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SuccessionPointer {
    pub old: ContainerId,
    pub new: ContainerId,
    pub sig: OwnerSig,
}

impl SuccessionPointer {
    fn canonical_bytes(old: &ContainerId, new: &ContainerId) -> Vec<u8> {
        serde_json::to_vec(&(old, new)).expect("succession pointer is serializable")
    }

    /// Owner-sign a succession `old → new`.
    pub fn sign(old: ContainerId, new: ContainerId, authority: &dyn SigningAuthority) -> Self {
        let sig = authority.sign(&Self::canonical_bytes(&old, &new));
        Self { old, new, sig }
    }

    /// Verify the owner signature over `(old, new)`.
    pub fn verify(&self, verifier: &dyn crate::policy::VerifyingAuthority) -> bool {
        verifier.verify(&Self::canonical_bytes(&self.old, &self.new), &self.sig)
    }
}

/// The owner-private alias ledger, backed by a dedicated non-replicated
/// `LoroDoc`.
pub struct AliasLedger {
    doc: NonReplicated<LoroDoc>,
    authority: Box<dyn SigningAuthority>,
}

impl AliasLedger {
    /// Open a fresh ledger owned by `peer`. The backing doc is
    /// [`NonReplicated`] — the C1 registry (Inc 3) will never replicate it.
    pub fn new(peer: StablePeerId, authority: Box<dyn SigningAuthority>) -> Self {
        let doc = LoroDoc::new();
        doc.set_peer_id(peer.0)
            .expect("fresh doc accepts a peer id before any op");
        Self {
            doc: NonReplicated::new(doc),
            authority,
        }
    }

    fn doc(&self) -> &LoroDoc {
        self.doc.owner_local()
    }

    /// Record a rotation: sign a container succession `old → new` and store the
    /// per-block id remapping the re-encode produced (`old_block → new_block`).
    /// Returns the signed succession pointer.
    pub fn record_rotation(
        &self,
        old_container: ContainerId,
        new_container: ContainerId,
        block_remap: &[(BlockId, BlockId)],
    ) -> SuccessionPointer {
        assert_ne!(
            old_container, new_container,
            "a container rotation must succeed to a DISTINCT fresh container id \
             (else recipients could correlate old↔new); got `{}` → itself",
            old_container.0
        );
        let pointer =
            SuccessionPointer::sign(old_container, new_container, self.authority.as_ref());

        let aliases = self.doc().get_map(ALIAS_MAP);
        for (old, new) in block_remap {
            assert_ne!(
                old, new,
                "a re-encode alias must map an OLD id to a DISTINCT fresh id \
                 (else recipients could correlate old↔new); got `{}` → itself",
                old.0
            );
            aliases
                .insert(old.0.as_str(), new.0.as_str())
                .expect("inserting a string alias cannot fail");
        }

        let successions = self.doc().get_list(SUCCESSION_LIST);
        let json = serde_json::to_string(&pointer).expect("pointer is serializable");
        successions
            .push(json.as_str())
            .expect("appending a string to a Loro list cannot fail");

        self.doc().commit();
        pointer
    }

    /// One-hop successor of `old` per the block-alias map, if it was rotated.
    fn alias_hop(&self, old: &BlockId) -> Option<BlockId> {
        let aliases = self.doc().get_map(ALIAS_MAP);
        aliases
            .get(old.0.as_str())
            .map(|v| {
                v.into_value()
                    .expect("alias value is a plain string")
                    .into_string()
                    .expect("alias value is a string")
            })
            .map(|s| BlockId(s.to_string()))
    }

    /// Resolve an old block id to its CURRENT id, following the chain across
    /// successive rotations (`old → mid → new`). Returns the input unchanged if
    /// it was never rotated. Panics loudly on a cycle (a re-encode never maps
    /// an id to an ancestor, so a cycle is a corrupted ledger, not a normal
    /// state).
    pub fn resolve(&self, id: &BlockId) -> BlockId {
        let mut current = id.clone();
        let mut seen = std::collections::BTreeSet::new();
        while let Some(next) = self.alias_hop(&current) {
            assert!(
                seen.insert(current.clone()),
                "alias ledger cycle detected while resolving `{}` — corrupted ledger",
                id.0
            );
            current = next;
        }
        current
    }

    /// One-hop container successor recorded for `old`, if any (the most recent
    /// wins if the same `old` were rotated twice — not expected, but defined).
    pub fn container_successor(&self, old: &ContainerId) -> Option<ContainerId> {
        let successions = self.doc().get_list(SUCCESSION_LIST);
        let mut found = None;
        for i in 0..successions.len() {
            let value = successions.get(i).expect("index < len is in bounds");
            let s = value
                .into_value()
                .expect("succession element is a plain value")
                .into_string()
                .expect("succession element is a string");
            let pointer: SuccessionPointer =
                serde_json::from_str(s.as_str()).expect("stored pointer round-trips");
            if &pointer.old == old {
                found = Some(pointer.new);
            }
        }
        found
    }

    /// Every `old → current` block mapping the ledger holds (terminal ids
    /// resolved through the chain). The input to
    /// [`owner_backlink_rewrite_sql`].
    pub fn block_aliases(&self) -> Vec<(BlockId, BlockId)> {
        let aliases = self.doc().get_map(ALIAS_MAP);
        let mut out = Vec::new();
        aliases.for_each(|old, _| {
            let old_id = BlockId(old.to_string());
            let current = self.resolve(&old_id);
            out.push((old_id, current));
        });
        out
    }

    /// SQL statements that rewrite the **owner's** `block_links` backlinks from
    /// rotated-out ids to their current ids, resolved through the ledger.
    ///
    /// Integration point (Inc 3 / registry wiring): holon's SQL layer applies
    /// these in the SAME transaction as the rotation, alongside the existing
    /// `block_links` writers in
    /// `crates/holon/src/core/sql_operation_provider.rs` (the junction whose
    /// `resolved_id` column these `UPDATE`s target). No signature of an
    /// existing writer changes — the ledger only *emits* statements.
    ///
    /// Recipients, lacking the ledger, are NOT rewritten: their backlinks to a
    /// vanished old id are NULLed by the ordinary block-delete path
    /// (`sql_operation_provider.rs`: `UPDATE block_links SET resolved_id = NULL
    /// WHERE resolved_id = '<gone>'`) and render as a loud unresolved ref — the
    /// existing dangling-link path, never a silent drop.
    pub fn owner_backlink_rewrite_sql(&self) -> Vec<String> {
        // SAFETY of string interpolation here: block/container ids are H6
        // mint-once stable ids (never user free-text) and `''`-escaped below,
        // mirroring the existing `sql_operation_provider.rs` block_links
        // writers. FOLLOW-UP (registry wiring, Inc 3): switch these to bound
        // parameters when the ledger executes through holon's SQL layer rather
        // than emitting statement strings, so the safety no longer rests on the
        // id-shape invariant.
        self.block_aliases()
            .into_iter()
            .filter(|(old, new)| old != new)
            .map(|(old, new)| {
                format!(
                    "UPDATE block_links SET resolved_id = '{}' WHERE resolved_id = '{}'",
                    new.0.replace('\'', "''"),
                    old.0.replace('\'', "''"),
                )
            })
            .collect()
    }
}
