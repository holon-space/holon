//! The **one-sided, cursor-based, blob-level transport seam** every
//! true-sharing backend implements (true-sharing plan §B).
//!
//! One trait covers the whole ratified ladder — an in-process relay (this
//! module), an HTTPS blind relay, and an iroh device mesh — because it commits
//! only to what all three can honor:
//!
//! - **One-sided.** `push` and `pull` each speak to ONE side's state. The
//!   two-sided `sync_pair(&LoroDoc, &LoroDoc)` shape (`multi_peer.rs`) is
//!   deliberately not copied: a relay never holds both peers' version vectors.
//! - **Blob-level.** [`Envelope::payload`] is opaque bytes. Encryption wraps
//!   the payload without touching this trait, and Loro's import is idempotent
//!   and order-tolerant, so a cursor-ordered append log is a sufficient wire
//!   model.
//! - **No `subscribe`.** Sync cadence belongs to the caller (a PBT transition,
//!   a foreground hook, a timer). Adding a push-stream later is additive.
//!
//! Pure-Rust and dependency-light on purpose: this module must build for
//! `target_os = "android"`, which excludes the desktop-only transport stacks.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use anyhow::Result;
use anyhow::anyhow;
use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

/// The reserved log id of the whole-vault root container — the id
/// [`crate::container_registry::ROOT_CONTAINER_ID`] replicates under.
pub const ROOT_LOG_ID: &str = "holon_tree";

/// The reserved log id of the owner-scoped crossing log (ADR 0028 H2). Carried
/// on the same transport as content containers so a receiver's arbitration sees
/// the same total order the owner appended.
pub const CROSSING_LOG_ID: &str = "__holon_crossing_log";

/// One append-only log on the transport = one container (or the reserved
/// crossing log). Parse-don't-validate: callers pass this, never a bare string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ContainerLogId(pub String);

impl ContainerLogId {
    pub fn root() -> Self {
        Self(ROOT_LOG_ID.to_string())
    }

    pub fn crossing_log() -> Self {
        Self(CROSSING_LOG_ID.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ContainerLogId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Stable per-device peer id — the sender identity on every envelope and the
/// second component of the ADR 0028 crossing-ordering tuple. Produced by
/// [`crate::share_peer_id::stable_peer_id`].
///
/// Lives here rather than in `holon-sharing` so the transport (which cannot
/// depend on the policy crate) and the acceptor name the SAME type;
/// `holon_sharing::types` re-exports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StablePeerId(pub u64);

/// What an envelope's payload is. Versioned as an enum so a relay written
/// against v1 rejects an unknown kind loudly instead of guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlobKind {
    /// An incremental Loro update (`ExportMode::updates`).
    Update,
    /// A full-state snapshot — the recovery path for a receiver whose version
    /// vector predates a compaction.
    Snapshot,
}

/// The membership claim a sender attaches so the RECEIVER can decide, without
/// asking anyone, whether to import. Verified against a lease chain by
/// `holon_sharing::acceptor::admit`; the relay never reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipProof {
    /// The principal the sender claims to be acting as.
    pub principal: String,
    /// The container selector the claim covers.
    pub selector: String,
    /// Sharing epoch the claim was minted under (ADR 0028 H2).
    pub epoch: u64,
    /// Serialized `MembershipChain` (owner→…→grantee certs). Opaque here — only
    /// the acceptor parses it.
    pub chain: Vec<u8>,
}

/// Signature over the envelope's canonical bytes under the CONTAINER key — the
/// reason a relay cannot inject content it did not receive from a member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobSig(pub Vec<u8>);

/// Hash-chain head over a log, reserved for fork detection (plan OQ3: signed
/// heads on every push). Populated from Inc4; carried from Inc0 so the wire
/// contract never has to change to add it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadHash(pub [u8; 32]);

/// One unit of transport. `payload` is OPAQUE: neither the transport nor the
/// relay may decode it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub container: ContainerLogId,
    /// Relay-assigned position in the log. `None` on push (the relay assigns);
    /// `Some` on everything a pull returns.
    pub seq: Option<u64>,
    pub kind: BlobKind,
    pub sender: StablePeerId,
    /// Opaque Loro bytes; ciphertext once the Inc4 encryption wrapper lands.
    pub payload: Vec<u8>,
    pub auth: MembershipProof,
    pub sig: BlobSig,
    /// Reserved fork-detection head (OQ3).
    pub head: Option<HeadHash>,
}

/// What a relay says about an accepted push.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushReceipt {
    /// The position the envelope landed at — the cursor value a puller reaches
    /// AFTER consuming it.
    pub seq: u64,
}

/// A reader's position in one container log. `Cursor::start()` reads from the
/// beginning; a pull returns the cursor to resume from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct Cursor(pub u64);

impl Cursor {
    pub fn start() -> Self {
        Self(0)
    }
}

/// The seam. Every backend (in-process, HTTPS relay, iroh) implements exactly
/// this.
///
/// **Relay contract** an implementation must satisfy, and that
/// [`InMemoryRelay`] is the executable specification of:
/// 1. `push` appends and assigns a strictly increasing per-container `seq`
///    starting at 1.
/// 2. `pull(c, cursor)` returns every envelope with `seq > cursor.0` in
///    ascending `seq`, plus the cursor to resume from (unchanged when empty).
/// 3. `list_logs` returns exactly the containers that have received a push.
/// 4. Payloads are never inspected, rewritten, or reordered.
///
/// Failures are enriched `Err`s (which container, which cursor) — an envelope
/// is never silently dropped.
#[async_trait]
pub trait SyncTransport: Send + Sync {
    async fn push(&self, envelope: Envelope) -> Result<PushReceipt>;
    async fn pull(
        &self,
        container: &ContainerLogId,
        cursor: Cursor,
    ) -> Result<(Vec<Envelope>, Cursor)>;
    async fn list_logs(&self) -> Result<Vec<ContainerLogId>>;
}

/// An in-process append-log relay — the Inc0 backend and the executable
/// statement of the relay contract above.
///
/// Blind by construction: it stores `Envelope`s whole and never reads
/// `payload`, `auth`, or `sig`. [`Self::witness`] counts calls so a test can
/// prove the sync path RAN and consulted the transport exactly N times, rather
/// than passing because nothing happened.
#[derive(Clone, Default)]
pub struct InMemoryRelay {
    logs: Arc<Mutex<BTreeMap<ContainerLogId, Vec<Envelope>>>>,
    witness: Arc<RelayWitness>,
}

/// Call counters over a relay — the executed-witness primitive. A "nothing was
/// transported" assertion is only meaningful next to a witness proving the
/// caller actually ran.
#[derive(Debug, Default)]
pub struct RelayWitness {
    pushes: AtomicU64,
    pulls: AtomicU64,
    lists: AtomicU64,
}

impl RelayWitness {
    pub fn pushes(&self) -> u64 {
        self.pushes.load(Ordering::SeqCst)
    }
    pub fn pulls(&self) -> u64 {
        self.pulls.load(Ordering::SeqCst)
    }
    pub fn lists(&self) -> u64 {
        self.lists.load(Ordering::SeqCst)
    }
    /// Every consultation of the transport, whatever the verb.
    pub fn total(&self) -> u64 {
        self.pushes() + self.pulls() + self.lists()
    }
}

impl InMemoryRelay {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn witness(&self) -> Arc<RelayWitness> {
        self.witness.clone()
    }

    /// Total envelopes the relay holds across all logs — the "was anything
    /// transported" observable.
    pub fn stored_envelopes(&self) -> usize {
        self.logs
            .lock()
            .expect("relay logs lock")
            .values()
            .map(Vec::len)
            .sum()
    }
}

#[async_trait]
impl SyncTransport for InMemoryRelay {
    async fn push(&self, mut envelope: Envelope) -> Result<PushReceipt> {
        self.witness.pushes.fetch_add(1, Ordering::SeqCst);
        if envelope.payload.is_empty() {
            return Err(anyhow!(
                "refusing to push an empty payload to container `{}` (sender {}): an empty \
                 envelope carries no state and would occupy a seq the receiver must still walk",
                envelope.container,
                envelope.sender.0
            ));
        }
        let mut logs = self.logs.lock().expect("relay logs lock");
        let log = logs.entry(envelope.container.clone()).or_default();
        let seq = log.len() as u64 + 1;
        envelope.seq = Some(seq);
        log.push(envelope);
        Ok(PushReceipt { seq })
    }

    async fn pull(
        &self,
        container: &ContainerLogId,
        cursor: Cursor,
    ) -> Result<(Vec<Envelope>, Cursor)> {
        self.witness.pulls.fetch_add(1, Ordering::SeqCst);
        let logs = self.logs.lock().expect("relay logs lock");
        let Some(log) = logs.get(container) else {
            // An unknown container is not an error: a receiver polls before the
            // owner's first push. It is an EMPTY read at the same cursor.
            return Ok((Vec::new(), cursor));
        };
        if cursor.0 > log.len() as u64 {
            return Err(anyhow!(
                "cursor {} is beyond the end of container `{container}` (log holds {} entries) — \
                 the reader's position does not belong to this log",
                cursor.0,
                log.len()
            ));
        }
        let batch: Vec<Envelope> = log[cursor.0 as usize..].to_vec();
        let next = Cursor(log.len() as u64);
        Ok((batch, next))
    }

    async fn list_logs(&self) -> Result<Vec<ContainerLogId>> {
        self.witness.lists.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .logs
            .lock()
            .expect("relay logs lock")
            .keys()
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(container: &str, payload: &[u8]) -> Envelope {
        Envelope {
            container: ContainerLogId(container.to_string()),
            seq: None,
            kind: BlobKind::Update,
            sender: StablePeerId(7),
            payload: payload.to_vec(),
            auth: MembershipProof {
                principal: "peer".into(),
                selector: container.into(),
                epoch: 0,
                chain: Vec::new(),
            },
            sig: BlobSig(vec![1]),
            head: None,
        }
    }

    #[tokio::test]
    async fn push_assigns_ascending_seq_and_pull_resumes() {
        let relay = InMemoryRelay::new();
        let c = ContainerLogId::root();
        assert_eq!(
            relay.push(envelope("holon_tree", b"a")).await.unwrap().seq,
            1
        );
        assert_eq!(
            relay.push(envelope("holon_tree", b"b")).await.unwrap().seq,
            2
        );

        let (batch, cursor) = relay.pull(&c, Cursor::start()).await.unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0].seq, Some(1));
        assert_eq!(cursor, Cursor(2));

        let (batch, cursor2) = relay.pull(&c, cursor).await.unwrap();
        assert!(batch.is_empty());
        assert_eq!(cursor2, cursor);
    }

    #[tokio::test]
    async fn pull_of_unknown_container_is_empty_not_an_error() {
        let relay = InMemoryRelay::new();
        let (batch, cursor) = relay
            .pull(&ContainerLogId("never-pushed".into()), Cursor::start())
            .await
            .unwrap();
        assert!(batch.is_empty());
        assert_eq!(cursor, Cursor::start());
        assert_eq!(relay.witness().pulls(), 1);
    }

    #[tokio::test]
    async fn empty_payload_and_out_of_range_cursor_fail_loud() {
        let relay = InMemoryRelay::new();
        assert!(relay.push(envelope("holon_tree", b"")).await.is_err());
        relay.push(envelope("holon_tree", b"a")).await.unwrap();
        assert!(
            relay
                .pull(&ContainerLogId::root(), Cursor(9))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn list_logs_reports_only_pushed_containers() {
        let relay = InMemoryRelay::new();
        relay.push(envelope("holon_tree", b"a")).await.unwrap();
        relay.push(envelope(CROSSING_LOG_ID, b"x")).await.unwrap();
        let mut logs = relay.list_logs().await.unwrap();
        logs.sort();
        assert_eq!(
            logs,
            vec![ContainerLogId::crossing_log(), ContainerLogId::root()]
        );
    }
}
