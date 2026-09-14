//! One round of remote-list sync: pull → reconcile → apply → push.
//!
//! The peer and the local rows are reached through the two traits below, so
//! this module holds the ORDER and the conflict handling and nothing else — the
//! same sequence runs against a real transport and against a fixture peer.
//!
//! There is no timer here. A cadence belongs to whatever calls this, and a loop
//! that re-enters a half-finished round is a worse failure than a round nobody
//! started.

use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;

use anyhow::Result;
use async_trait::async_trait;
use holon_api::Value;

use crate::reconcile::LocalIntent;
use crate::reconcile::PushIntent;
use crate::reconcile::RemoteListReconciler;
use crate::snapshot::ListSnapshot;
use crate::snapshot::LocalRowReader;
use crate::spec::RowKey;

/// The two intents a reconciler can push.
///
/// These are HOLON's names, not any peer's. What a given system spells them as
/// lives in its sidecar's `request` mapping; this string only has to be stable
/// enough for that mapping to select on, and for the idempotency key to hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandVerb {
    Add,
    Remove,
}

impl CommandVerb {
    pub fn as_intent(&self) -> &'static str {
        match self {
            CommandVerb::Add => "add",
            CommandVerb::Remove => "remove",
        }
    }
}

/// One entry of a commit batch.
#[derive(Debug, Clone, PartialEq)]
pub struct CommitCommand {
    pub verb: CommandVerb,
    pub key: RowKey,
    /// The row the command is about, for a `request` mapping that needs more
    /// than the key — a peer adding an item wants its columns, and even a
    /// removal has to name the record in the peer's own vocabulary.
    pub columns: BTreeMap<String, Value>,
    /// The idempotency and ordering key, `<round_ms>_<16 hex>`.
    pub id: String,
}

impl CommitCommand {
    /// This command as one row line of the write leg's stream.
    fn to_row(&self, command_row_type: &str) -> Result<serde_json::Value> {
        let mut row = serde_json::Map::new();
        for (column, value) in &self.columns {
            row.insert(column.clone(), serde_json::to_value(value)?);
        }
        row.insert("id".into(), serde_json::json!(self.id));
        row.insert("verb".into(), serde_json::json!(self.verb.as_intent()));
        row.insert("key".into(), self.key.as_json().clone());
        row.insert("command_id".into(), serde_json::json!(self.id));
        Ok(serde_json::json!({ "type": command_row_type, "row": row }))
    }
}

/// One commit request: the version envelope it is based on, who is committing,
/// and the ordered commands.
#[derive(Debug, Clone, PartialEq)]
pub struct CommitBatch {
    /// The pull's version envelope, verbatim. Nothing here reads it; the
    /// sidecar's `request` mapping selects whichever of its fields the peer's
    /// own commit envelope wants.
    pub envelope: BTreeMap<String, Value>,
    /// The one number this crate compares, lifted out of the envelope.
    pub version: i64,
    pub device_id: String,
    pub commands: Vec<CommitCommand>,
}

impl CommitBatch {
    /// Turn the reconciler's push intents into commands based on `snapshot`.
    ///
    /// `round_ms` scopes the ids to ONE sync round; taking it as an argument
    /// rather than reading the clock keeps a batch reproducible from its
    /// inputs.
    ///
    /// **The id is derived from the command, never from its position or from
    /// the attempt.** A round that commits twice — because its verifying
    /// re-pull did not show the first commit — re-sends the same logical
    /// command, and the id is the peer's only means of recognising it. A
    /// positional or attempt-seeded id changes between the two sends and
    /// defeats exactly the deduplication it exists for, adding the row twice
    /// (`docs/Testing/bugfunnel/entries/
    /// 2026-09-01-shopping-retry-remints-idempotency-key.md`).
    pub fn from_push_intents(
        push: &[PushIntent],
        snapshot: &ListSnapshot,
        device_id: &str,
        round_ms: i64,
    ) -> Self {
        let commands = push
            .iter()
            .map(|intent| {
                let (verb, key, columns) = match intent {
                    PushIntent::Add { key, row } => {
                        (CommandVerb::Add, key.clone(), row.columns.clone())
                    }
                    PushIntent::Remove { key, row } => {
                        (CommandVerb::Remove, key.clone(), row.columns.clone())
                    }
                };
                let id = command_id(round_ms, verb, &key);
                CommitCommand {
                    verb,
                    key,
                    columns,
                    id,
                }
            })
            .collect();
        Self {
            envelope: snapshot.envelope().clone(),
            version: snapshot.version(),
            device_id: device_id.to_string(),
            commands,
        }
    }

    /// This batch as a row stream, for the connection's `request` mapping to
    /// turn into the peer's own envelope.
    ///
    /// The peer's own spelling of these commands is NOT here. It lives in that
    /// peer's sidecar. What travels is the intent, under Holon's name for it.
    ///
    /// No `scopes`: a scope says which rows one FILE owns so a replacement
    /// cannot delete another file's, and a request replaces nothing. The
    /// `request` mapping reads `rows` alone.
    pub fn to_row_stream(&self, spec: &crate::spec::ListSyncSpec) -> Result<serde_json::Value> {
        let mut batch_row = serde_json::Map::new();
        for (column, value) in &self.envelope {
            batch_row.insert(column.clone(), serde_json::to_value(value)?);
        }
        batch_row.insert("device_id".into(), serde_json::json!(self.device_id));

        let mut rows = vec![serde_json::json!({
            "type": spec.batch_row_type,
            "row": batch_row,
        })];
        for command in &self.commands {
            rows.push(command.to_row(&spec.command_row_type)?);
        }
        Ok(serde_json::json!({ "rows": rows }))
    }
}

/// `<round_ms>_<hash of the logical command>` — an epoch-scoped id whose second
/// half identifies WHAT is being asked rather than where it sat in one batch.
///
/// Two sends of the same logical command inside one round therefore carry one
/// id, and the same command in a LATER round carries a different one — which is
/// correct: that is a new intent, decided against a list read since.
fn command_id(round_ms: i64, verb: CommandVerb, key: &RowKey) -> String {
    let mut hasher = DefaultHasher::new();
    verb.as_intent().hash(&mut hasher);
    key.as_json().to_string().hash(&mut hasher);
    format!("{round_ms}_{:016x}", hasher.finish())
}

/// What a commit answered: the version the next commit must be based on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitAck {
    pub version: i64,
}

/// A remote list, as one round needs it.
#[async_trait]
pub trait RemoteListPeer: Send + Sync {
    /// One complete list fetch. A partial or failed fetch must fail here rather
    /// than yield a snapshot: absence inside a [`ListSnapshot`] is read as
    /// deletion.
    ///
    /// The connection supplies whatever arguments its own `pull_tool` declares
    /// placeholders for. Two names every list peer can rely on: `version` (the
    /// highest version observed so far, 0 before the first pull) and
    /// `device_id`. A third, `nocache`, is there only for a connection whose
    /// `cache_buster` declares it.
    async fn pull(&self) -> Result<ListSnapshot>;

    /// Apply a batch and answer the version the next commit is based on.
    async fn commit(&self, batch: &CommitBatch) -> Result<CommitAck>;
}

/// What one round decided and did.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncOutcome {
    /// Local writes, in order, for the dispatcher to execute.
    pub local: Vec<LocalIntent>,
    pub pulled: usize,
    /// Commands sent across every commit this round made.
    pub committed: usize,
    /// True when the verifying re-pull found pushes still outstanding and a
    /// second commit was needed.
    pub retried: bool,
}

/// Run one round: pull the list, reconcile it against the local rows, commit
/// what the peer is missing, and return the local writes.
///
/// **Version handling.** Every commit is based on the envelope the pull it
/// followed returned — never on a version nobody read. What proves the commit
/// landed is NOT the ack: a peer answers a new version whether or not someone
/// else wrote in between, so the number alone cannot tell "applied on top of
/// what I read" from "applied on top of something newer". The round therefore
/// re-pulls and lets the reconciler decide — if the pushes are gone, the commit
/// landed; if any remain, the list moved under us and the next commit is based
/// on the version just read. Two commits at most, then a loud failure: a sync
/// that cannot converge must not keep writing, and re-sending a batch over a
/// list it has not read is the blind overwrite this exists to prevent.
pub async fn sync_once(
    peer: &dyn RemoteListPeer,
    rows: &dyn LocalRowReader,
    reconciler: &RemoteListReconciler,
    device_id: &str,
    now_ms: i64,
) -> Result<SyncOutcome> {
    const MAX_COMMITS: i64 = 2;

    let local = rows.load().await?;
    let mut snapshot = peer.pull().await?;
    let mut committed = 0;
    let mut retried = false;

    for attempt in 0..=MAX_COMMITS {
        let outcome = reconciler.reconcile(&local, &snapshot)?;
        if outcome.push.is_empty() {
            return Ok(SyncOutcome {
                local: outcome.local,
                pulled: snapshot.len(),
                committed,
                retried,
            });
        }
        anyhow::ensure!(
            attempt < MAX_COMMITS,
            "remote list sync: {} change(s) still un-pushed after {MAX_COMMITS} commits against \
             versions up to {}; the list is being written faster than a round can converge, and \
             re-sending over a list this round has not read would lose whoever else is editing",
            outcome.push.len(),
            snapshot.version()
        );
        retried = attempt > 0;

        // `now_ms`, not `now_ms + attempt`: every commit this round makes must
        // mint the SAME id for the same logical command, or the peer cannot
        // recognise the re-send.
        let batch = CommitBatch::from_push_intents(&outcome.push, &snapshot, device_id, now_ms);
        let acked = peer.commit(&batch).await?;
        anyhow::ensure!(
            acked.version >= batch.version,
            "remote list sync: the peer answered version {} to a commit based on version {}; a \
             list that goes backwards is a peer bug, not a conflict to retry",
            acked.version,
            batch.version
        );
        committed += batch.commands.len();
        snapshot = pull_at_least(peer, acked.version).await?;
    }
    unreachable!("the loop returns or fails on its last pass")
}

/// Pull until the list is at least as new as a write we KNOW landed.
///
/// The verifying pull decides whether to commit again, so a cached response
/// older than our own commit would report "it did not land" about a write that
/// did — and the round would re-send it. A snapshot older than `floor` is
/// provably stale and is never reconciled against; a peer whose pull takes a
/// cache-buster argument serves a fresh fetch rather than the same cached body.
/// Bounded, then loud: a peer that cannot serve its own last write is a
/// condition to report, not to write more on top of.
async fn pull_at_least(peer: &dyn RemoteListPeer, floor: i64) -> Result<ListSnapshot> {
    const MAX_PULL_ATTEMPTS: usize = 3;

    let mut last_seen = None;
    for _ in 0..MAX_PULL_ATTEMPTS {
        let snapshot = peer.pull().await?;
        if snapshot.version() >= floor {
            return Ok(snapshot);
        }
        last_seen = Some(snapshot.version());
    }
    anyhow::bail!(
        "remote list sync: after a commit the peer answered version {floor}, but \
         {MAX_PULL_ATTEMPTS} reads still returned version {}; the list this round would decide \
         against is older than a write it just made, and acting on it would re-send that write",
        last_seen
            .map(|v| v.to_string())
            .unwrap_or_else(|| "?".to_string())
    )
}
