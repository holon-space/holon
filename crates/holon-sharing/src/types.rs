//! Domain types for the ADR 0028 owner-scoped crossing log.
//!
//! Parse-don't-validate: the log speaks in typed identities (`BlockId`,
//! `ContainerId`, `CrossingId`, `StablePeerId`), never bare strings passed
//! around and re-checked at every call site.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Serialize;

/// A block that can cross container boundaries.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BlockId(pub String);

/// An owner-scoped container = one shared subtree / `LoroDoc` (ADR 0028 §5).
/// A crossing moves a block from one container to another; a `TreeID` move
/// never spans containers, so this is the only cross-doc primitive.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ContainerId(pub String);

/// Client-minted stable id for ONE crossing (the repo keeps inverse-ops /
/// client-minted-op-ids warm — ADR §7). The `Begin` and `Commit` brackets of a
/// crossing share this id; an undo references it via [`Crossing::inverse_of`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CrossingId(pub String);

impl std::fmt::Display for CrossingId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Client-minted stable id for one policy-overlay edit.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PolicyEditId(pub String);

/// Stable per-device peer id — second component of the frozen ordering tuple
/// (Inc 0(b)). Produced by `holon_loro::share_peer_id::stable_peer_id`
/// (blake3 of device key × share × generation). The log stores the `u64` so
/// the library need not depend on the device-key type; the harness supplies
/// the real value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StablePeerId(pub u64);

/// FROZEN ordering tuple (Inc 0(b), ratified):
/// `(lamport_height(owner_log_doc) at append, stable_peer_id)`, ascending,
/// **max = latest crossing wins**. `(lamport, peer)` is a strict total order
/// over concurrent crossings (proven 100/100 on real substrate at Inc 0; the
/// peer tiebreak was load-bearing in 50/100), so no third component is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CrossingKey {
    pub lamport: u32,
    pub peer: StablePeerId,
}

/// Materializable block payload. Carried on a [`Crossing`] so a loud-rejected
/// loser (D2) can be returned as a *keepable divergent copy* — a real value the
/// caller can materialize, not merely an error string.
///
/// DISCLOSED: this is the Inc-4 stand-in shape. The real block model (marks,
/// children, edge fields) binds when the container registry lands (Inc 3);
/// the crossing carries whatever the migration needs to re-create the block in
/// the target container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockContent {
    pub text: String,
    pub props: BTreeMap<String, String>,
}

/// One boundary crossing: a **delete-in-source + create-in-target pair**
/// bracketed by log entries (ADR 0028 H2). `widens_audience` is the
/// conservative static upper bound copied from
/// [`holon_api::BoundaryBehavior::Crossing`]; it drives both the D1
/// explicit-confirm gate and the leak-direction of the migration journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Crossing {
    pub crossing_id: CrossingId,
    pub block: BlockId,
    pub source: ContainerId,
    pub target: ContainerId,
    pub widens_audience: bool,
    pub content: BlockContent,
    /// Set when this crossing is the inverse (undo) of a prior *committed*
    /// crossing (ADR §7 / review S6(b)).
    pub inverse_of: Option<CrossingId>,
    /// The crossing this author believed was the block's CURRENT one when
    /// authoring (the observed history). `None` = the block had no prior
    /// crossing in the author's view. This is the causal-order witness that
    /// distinguishes a **sequential re-move** (author observed the prior
    /// crossing ⇒ supersedes it ⇒ both legitimate) from a **concurrent
    /// divergent** crossing (neither observed the other ⇒ they contend, D2).
    /// Migration drivers set this to the currently-winning crossing for the
    /// block; two offline devices both see `None` from a shared base.
    ///
    /// **Same-block invariant.** A witness may only reference a crossing of the
    /// SAME block. A witness naming an unknown id, a crossing on a different
    /// block, or forming a cycle is MALFORMED and is rejected loudly — never
    /// silently honored (honoring it would erase an unrelated block's crossing)
    /// and never silently dropped. Validated at the source
    /// ([`crate::log::CrossingLog::append_crossing`]) and defensively at the
    /// read boundary ([`crate::arbitration::arbitrate`]).
    ///
    /// **Trust model.** `supersedes` is owner-scope-*trusted* input: it is
    /// written by the owner's own migration drivers and the whole log lives
    /// inside one owner's vault. A lying or buggy owner device can only corrupt
    /// *its own* vault's arbitration — never another owner's — and, after the
    /// validation above, only LOUDLY (as a malformed rejection with a keepable
    /// copy), never as silent data loss.
    pub supersedes: Option<CrossingId>,
}

/// A policy-overlay mutation (grant / revoke / lease). Enters the ONE log
/// alongside crossings under a single arbitration rule (review A1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyEdit {
    pub edit_id: PolicyEditId,
    pub container: ContainerId,
    pub principal: String,
    pub change: PolicyChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyChange {
    Grant,
    Revoke,
    Lease,
}

/// One appended log record: the frozen-tuple key, the body, and a
/// structurally-present signature over `(key, body)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub key: CrossingKey,
    pub body: LogEntryBody,
    pub sig: OwnerSig,
}

/// What a log entry records. Crossings are bracketed: a `CrossingBegin`
/// (delete-in-source about to apply) and a later `CrossingCommit`
/// (create-in-target applied). Policy edits are single entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LogEntryBody {
    /// Opening bracket — records the full crossing intent; the migration window
    /// opens here.
    CrossingBegin(Crossing),
    /// Closing bracket — the crossing named by `crossing_id` is committed
    /// (create-in-target done). Absence of a matching commit marks an
    /// uncommitted (contended or crashed-mid-window) crossing.
    CrossingCommit { crossing_id: CrossingId },
    /// A policy-overlay mutation, in the same log under the same arbitration.
    PolicyEdit(PolicyEdit),
}

/// Opaque owner-identity signature (OQ4 ratified — a *distinct* owner-identity
/// key, custody/recovery pending the W4 enrollment-ceremony ruling).
///
/// DISCLOSED: at Inc 4 the signature is **structurally present but not yet
/// verifiable** — real key wiring lands with Inc 5. Committing the field shape
/// now means Inc 5 swaps [`SigningAuthority`]'s impl without a log-layout
/// change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerSig(pub Vec<u8>);

/// Seam for the owner-identity key (OQ4). Device keys are transport/session
/// identity only; this trait signs durable authority objects (crossings,
/// policy edits). Non-owner devices will sign via D4 short-lived
/// owner-delegation certs — wired at Inc 5.
pub trait SigningAuthority {
    /// Sign the canonical bytes of a log entry.
    fn sign(&self, payload: &[u8]) -> OwnerSig;
}

/// Inc-4 placeholder authority: blake3-hashes the payload as a stand-in
/// signature.
///
/// DISCLOSED: this does **not** verify owner identity — it only makes entries
/// signed-*shaped*. Real key custody lands with Inc 5 (OQ4 owner-identity key).
/// A verifier that ran today would find every entry "signed" by an unowned key;
/// callers MUST NOT treat an `UnverifiedAuthority` signature as authorization.
pub struct UnverifiedAuthority;

impl SigningAuthority for UnverifiedAuthority {
    fn sign(&self, payload: &[u8]) -> OwnerSig {
        OwnerSig(blake3::hash(payload).as_bytes().to_vec())
    }
}
