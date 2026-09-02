//! `sync_once` — the caller-driven orchestrator that moves one round of state
//! between a [`ContainerRegistry`] and a [`SyncTransport`].
//!
//! Deliberately **not** a background loop: cadence belongs to the caller (a PBT
//! `SyncNow` transition, a foreground hook, a timer), which is why the
//! transport trait has no `subscribe`. One call = one bounded round.
//!
//! ## Whole-vault = registry iteration
//! There is no "share the whole vault" mode distinct from per-container
//! sharing. The whole vault IS every container in
//! [`ContainerRegistry::replication_set`] — the C1 replicate-all set, which has
//! no filter parameter — so whole-vault sync is the degenerate case of the same
//! loop, with per-container keys and epochs intact.
//!
//! ## Nothing is dropped
//! Transport breakage is an enriched `Err`. An unauthorized envelope is a typed
//! [`AdmitDecision`] recorded in [`SyncReport::refusals`]. An envelope for a
//! container this side has not mounted is recorded in
//! [`SyncReport::unmounted`] — never skipped in silence.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use anyhow::Context;
use anyhow::Result;
use holon_loro::container_registry::ContainerRegistry;
use holon_loro::sync_transport::BlobKind;
use holon_loro::sync_transport::BlobSig;
use holon_loro::sync_transport::ContainerLogId;
use holon_loro::sync_transport::Cursor;
use holon_loro::sync_transport::Envelope;
use holon_loro::sync_transport::MembershipProof;
use holon_loro::sync_transport::StablePeerId;
use holon_loro::sync_transport::SyncTransport;

use crate::acceptor::AcceptorContext;
use crate::acceptor::AdmitDecision;
use crate::acceptor::admit;
use crate::acceptor::blob_canonical_bytes;
use crate::acceptor::encode_chain;
use crate::lease::MembershipChain;
use crate::policy::Principal;

/// What a publisher attaches to everything it pushes: who it is, the audience
/// the blob is destined for, and that audience's owner-issued cert chain.
#[derive(Clone)]
pub struct OutboundAuth {
    pub sender: StablePeerId,
    /// The principal on the OTHER end of this round — the peer that will admit
    /// these blobs. It is a function of the round's direction, never a
    /// constant: pushing to the owner means `audience` is the OWNER even
    /// though `chain` still proves this device. See [`crate::acceptor`].
    pub audience: Principal,
    pub epoch: u64,
    /// This device's own owner-signed chain. Its terminal grantee is the
    /// subject the admitter checks capabilities against.
    pub chain: MembershipChain,
}

/// Per-container sync bookkeeping: how far this side has published, and how far
/// it has read.
#[derive(Default, Clone)]
struct ContainerSyncState {
    /// Version vector at the last successful push; `None` = never published.
    last_pushed: Option<loro::VersionVector>,
    cursor: Cursor,
}

/// One side's durable-enough sync position across all containers. Held by the
/// caller across rounds; a fresh session re-publishes and re-reads from the
/// start (correct, because import is idempotent).
#[derive(Default)]
pub struct SyncSession {
    states: BTreeMap<ContainerLogId, ContainerSyncState>,
}

impl SyncSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cursor(&self, container: &ContainerLogId) -> Cursor {
        self.states
            .get(container)
            .map(|s| s.cursor)
            .unwrap_or_default()
    }
}

/// What one round did. Every outcome — including every refusal — is here, so a
/// caller can assert on the round rather than infer it from side effects.
#[derive(Debug, Default, Clone)]
pub struct SyncReport {
    /// Containers the round walked (the non-vacuity witness: a round over an
    /// empty registry is visible as 0).
    pub containers_visited: usize,
    /// `(container, relay-assigned seq)` per accepted push.
    pub pushed: Vec<(ContainerLogId, u64)>,
    /// Containers that had no new local state to publish.
    pub skipped_no_delta: Vec<ContainerLogId>,
    /// `(container, payload bytes)` per imported envelope.
    pub imported: Vec<(ContainerLogId, usize)>,
    /// Every envelope the acceptor turned down, with its typed decision.
    pub refusals: Vec<(ContainerLogId, AdmitDecision)>,
    /// Logs the transport holds that this side has no container mounted for.
    pub unmounted: Vec<ContainerLogId>,
    /// Containers NOT published because the publisher holds no membership proof
    /// to attach. A typed, reported skip — the relay is untrusted, so state
    /// must never leave this device under an unproven claim.
    pub unauthorized: Vec<ContainerLogId>,
}

impl SyncReport {
    fn absorb(&mut self, other: SyncReport) {
        self.containers_visited = self.containers_visited.max(other.containers_visited);
        self.pushed.extend(other.pushed);
        self.skipped_no_delta.extend(other.skipped_no_delta);
        self.imported.extend(other.imported);
        self.refusals.extend(other.refusals);
        self.unmounted.extend(other.unmounted);
        self.unauthorized.extend(other.unauthorized);
    }
}

/// Publish every container's un-pushed delta, then read and admit everything
/// waiting. One bounded round.
pub async fn sync_once(
    registry: &ContainerRegistry,
    transport: &dyn SyncTransport,
    session: &mut SyncSession,
    auth: &OutboundAuth,
    ctx: &AcceptorContext<'_>,
) -> Result<SyncReport> {
    let mut report = push_once(registry, transport, session, auth).await?;
    report.absorb(pull_once(registry, transport, session, ctx).await?);
    Ok(report)
}

/// The publish half of a round.
pub async fn push_once(
    registry: &ContainerRegistry,
    transport: &dyn SyncTransport,
    session: &mut SyncSession,
    auth: &OutboundAuth,
) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    let containers = registry
        .replication_set()
        .await
        .context("reading the replication set to publish it")?;
    report.containers_visited = containers.len();

    let authorized = !auth.chain.certs.is_empty();
    for container in containers {
        let log = ContainerLogId(container.id.clone());
        if !authorized {
            // No proof to attach ⇒ nothing leaves this device. The relay is
            // untrusted store-and-forward, so an unprovable push would put state
            // in front of it for no admissible reason.
            report.unauthorized.push(log);
            continue;
        }
        let state = session.states.entry(log.clone()).or_default();
        // ALLOW(loro_doc_escape): `doc` is re-read after the `transport.push` await
        // (line below reads `doc.oplog_vv()` post-push) — it can't be confined to a
        // single with_read/with_write scope without changing what vv gets recorded.
        let doc = container.doc.doc();
        doc.commit();
        let from = state.last_pushed.clone().unwrap_or_default();
        let payload = doc
            .export(loro::ExportMode::updates_owned(from))
            .with_context(|| format!("exporting the un-pushed delta of container `{log}`"))?;
        if payload.is_empty() {
            report.skipped_no_delta.push(log);
            continue;
        }

        let mut envelope = Envelope {
            container: log.clone(),
            seq: None,
            kind: BlobKind::Update,
            sender: auth.sender,
            payload,
            auth: MembershipProof {
                audience: auth.audience.0.clone(),
                selector: container.id.clone(),
                epoch: auth.epoch,
                chain: encode_chain(&auth.chain),
            },
            sig: BlobSig(Vec::new()),
            head: None,
        };
        envelope.sig = BlobSig(blob_canonical_bytes(&envelope));

        let receipt = transport
            .push(envelope)
            .await
            .with_context(|| format!("pushing container `{log}` to the transport"))?;
        state.last_pushed = Some(doc.oplog_vv());
        report.pushed.push((log, receipt.seq));
    }
    Ok(report)
}

/// The read-and-admit half of a round.
pub async fn pull_once(
    registry: &ContainerRegistry,
    transport: &dyn SyncTransport,
    session: &mut SyncSession,
    ctx: &AcceptorContext<'_>,
) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    let containers = registry
        .replication_set()
        .await
        .context("reading the replication set to fill it")?;
    report.containers_visited = containers.len();
    let mounted: BTreeSet<ContainerLogId> = containers
        .iter()
        .map(|c| ContainerLogId(c.id.clone()))
        .collect();

    for container in containers {
        let log = ContainerLogId(container.id.clone());
        let cursor = session.states.entry(log.clone()).or_default().cursor;
        let (batch, next) = transport
            .pull(&log, cursor)
            .await
            .with_context(|| format!("pulling container `{log}` from cursor {}", cursor.0))?;

        for envelope in batch {
            match admit(&envelope, ctx) {
                AdmitDecision::Import { .. } => {
                    let bytes = envelope.payload.len();
                    container
                        .doc
                        .apply_update_with_origin("sync_import", &envelope.payload)
                        .with_context(|| {
                            format!(
                                "importing an ADMITTED {bytes}-byte blob (seq {:?}) into container \
                                 `{log}` — admission succeeded, so a failure here is corruption, \
                                 not authorization",
                                envelope.seq
                            )
                        })?;
                    report.imported.push((log.clone(), bytes));
                }
                refusal => report.refusals.push((log.clone(), refusal)),
            }
        }
        session.states.entry(log).or_default().cursor = next;
    }

    for log in transport
        .list_logs()
        .await
        .context("enumerating the transport's logs to detect unmounted containers")?
    {
        if !mounted.contains(&log) {
            report.unmounted.push(log);
        }
    }
    Ok(report)
}
