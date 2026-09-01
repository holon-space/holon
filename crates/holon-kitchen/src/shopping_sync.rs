//! One round of shopping-list sync: pull → reconcile → apply → push.
//!
//! The peer and the local rows are reached through the two traits below so this
//! module holds the ORDER and the conflict handling and nothing else — the same
//! sequence runs against the real `rest` transport and against a mock peer.
//!
//! There is no timer here. A cadence belongs to whatever calls this, and a loop
//! that re-enters a half-finished round is a worse failure than a round nobody
//! started.

use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;

use anyhow::Result;
use async_trait::async_trait;
use holon_api::Operation;
use holon_api::Value;

use crate::shopping::CompleteSnapshot;
use crate::shopping::ItemKey;
use crate::shopping::ListVersion;
use crate::shopping::LocalIntent;
use crate::shopping::LocalShoppingItem;
use crate::shopping::PushIntent;
use crate::shopping::ShoppingReconciler;

/// The entity every local intent is written against.
pub const SHOPPING_ITEM_ENTITY: &str = "shopping_item";

/// The `/commit` verbs the capture pinned. A check toggle is deliberately
/// absent: see [`PushIntent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandVerb {
    Add,
    Del,
}

impl CommandVerb {
    fn as_wire(&self) -> &'static str {
        match self {
            CommandVerb::Add => "add",
            CommandVerb::Del => "del",
        }
    }
}

/// One entry of the `/commit` envelope's `commands` array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitCommand {
    pub verb: CommandVerb,
    pub key: ItemKey,
    /// `<epoch_ms>_<seq>`, the peer's idempotency and ordering key.
    pub id: String,
}

impl CommitCommand {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "cmd": self.verb.as_wire(),
            "good": { "name": self.key.name, "cat": self.key.cat, "new": true },
            "id": self.id,
        })
    }
}

/// One `/commit` request: the versions it is based on, who is committing, and
/// the ordered commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitBatch {
    pub old_version: i64,
    pub old_picked_items_version: i64,
    pub device_id: String,
    pub commands: Vec<CommitCommand>,
}

impl CommitBatch {
    /// Turn the reconciler's push intents into commands based on `version`.
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
    /// defeats exactly the deduplication it exists for, adding the item
    /// twice (`docs/Testing/bugfunnel/entries/
    /// 2026-09-01-shopping-retry-remints-idempotency-key.md`).
    pub fn from_push_intents(
        push: &[PushIntent],
        version: ListVersion,
        device_id: &str,
        round_ms: i64,
    ) -> Self {
        let commands = push
            .iter()
            .map(|intent| {
                let (verb, key) = match intent {
                    PushIntent::Add(row) => (CommandVerb::Add, row.key()),
                    PushIntent::Remove { key, .. } => (CommandVerb::Del, key.clone()),
                };
                let id = command_id(round_ms, verb, &key);
                CommitCommand { verb, key, id }
            })
            .collect();
        Self {
            old_version: version.list,
            old_picked_items_version: version.picked,
            device_id: device_id.to_string(),
            commands,
        }
    }

    pub fn commands_json(&self) -> serde_json::Value {
        serde_json::Value::Array(self.commands.iter().map(CommitCommand::to_json).collect())
    }
}

/// `<round_ms>_<hash of the logical command>` — the peer's documented
/// `<epoch_ms>_<seq>` shape, with the sequence number replaced by something
/// that identifies WHAT is being asked rather than where it sat in one batch.
///
/// Two sends of the same logical command inside one round therefore carry one
/// id, and the same command in a LATER round carries a different one — which is
/// correct: that is a new intent, decided against a list read since.
fn command_id(round_ms: i64, verb: CommandVerb, key: &ItemKey) -> String {
    let mut hasher = DefaultHasher::new();
    verb.as_wire().hash(&mut hasher);
    key.hash(&mut hasher);
    format!("{round_ms}_{:016x}", hasher.finish())
}

/// What the peer answered a commit with: the versions the NEXT commit must be
/// based on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitAck {
    pub version: i64,
    pub picked_items_version: i64,
}

/// The shopping-list peer, as this module needs it.
#[async_trait]
pub trait ShoppingPeer: Send + Sync {
    /// One complete list fetch. A partial or failed fetch must fail here rather
    /// than yield a snapshot: absence inside a [`CompleteSnapshot`] is read as
    /// deletion.
    async fn pull(&self) -> Result<CompleteSnapshot>;

    async fn commit(&self, batch: &CommitBatch) -> Result<CommitAck>;
}

/// The local `shopping_item` rows, read-only. Writing them is the dispatcher's
/// job, through the follow-up operations [`local_intent_operation`] builds.
#[async_trait]
pub trait ShoppingRowReader: Send + Sync {
    async fn load(&self) -> Result<Vec<LocalShoppingItem>>;
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
/// **Version handling.** Every commit is based on the version the pull it
/// followed returned — never on a version nobody read. What proves the commit
/// landed is NOT the ack: the peer answers a new version whether or not someone
/// else wrote in between, so the number alone cannot tell "applied on top of
/// what I read" from "applied on top of something newer". The round therefore
/// re-pulls and lets the reconciler decide — if the pushes are gone, the commit
/// landed; if any remain, the list moved under us and the next commit is based
/// on the version just read. Two commits at most, then a loud failure: a sync
/// that cannot converge must not keep writing, and re-sending a batch over a
/// list it has not read is the blind overwrite this exists to prevent.
pub async fn sync_once(
    peer: &dyn ShoppingPeer,
    rows: &dyn ShoppingRowReader,
    reconciler: &ShoppingReconciler,
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
            "shopping sync: {} change(s) still un-pushed after {MAX_COMMITS} commits against \
             versions up to {}; the list is being written faster than a round can converge, and \
             re-sending over a list this round has not read would lose whoever else is editing",
            outcome.push.len(),
            snapshot.version().list
        );
        retried = attempt > 0;

        // `now_ms`, not `now_ms + attempt`: every commit this round makes must
        // mint the SAME id for the same logical command, or the peer cannot
        // recognise the re-send.
        let batch =
            CommitBatch::from_push_intents(&outcome.push, snapshot.version(), device_id, now_ms);
        let ack = peer.commit(&batch).await?;
        anyhow::ensure!(
            ack.version >= batch.old_version,
            "shopping sync: the peer answered version {} to a commit based on version {}; a list \
             that goes backwards is a peer bug, not a conflict to retry",
            ack.version,
            batch.old_version
        );
        committed += batch.commands.len();
        snapshot = pull_at_least(peer, ack.version).await?;
    }
    unreachable!("the loop returns or fails on its last pass")
}

/// Pull until the list is at least as new as a write we KNOW landed.
///
/// The verifying pull decides whether to commit again, so a cached response
/// older than our own commit would report "it did not land" about a write that
/// did — and the round would re-send it. A snapshot older than `floor` is
/// provably stale and is never reconciled against; the request carries a
/// cache-buster (`shopping_rest`), so a retry is a fresh fetch rather than the
/// same cached body. Bounded, then loud: a peer that cannot serve its own last
/// write is a condition to report, not to write more on top of.
async fn pull_at_least(peer: &dyn ShoppingPeer, floor: i64) -> Result<CompleteSnapshot> {
    const MAX_PULL_ATTEMPTS: usize = 3;

    let mut last_seen = None;
    for _ in 0..MAX_PULL_ATTEMPTS {
        let snapshot = peer.pull().await?;
        if snapshot.version().list >= floor {
            return Ok(snapshot);
        }
        last_seen = Some(snapshot.version().list);
    }
    anyhow::bail!(
        "shopping sync: after a commit the peer answered version {floor}, but {MAX_PULL_ATTEMPTS} \
         reads still returned version {}; the list this round would decide against is older than \
         a write it just made, and acting on it would re-send that write",
        last_seen
            .map(|v| v.to_string())
            .unwrap_or_else(|| "?".to_string())
    )
}

/// The generic operation that performs one local intent.
///
/// Every local write goes through the declared type's own `create`/`set_field`/
/// `delete` authority — the shopping sync adds no second writer of its own.
pub fn local_intent_operation(intent: &LocalIntent) -> Operation {
    match intent {
        LocalIntent::Insert(row) => Operation::new(
            SHOPPING_ITEM_ENTITY,
            "create",
            "Add shopping item",
            insert_params(row),
        ),
        LocalIntent::SetCount { id, count } => set_field(
            id,
            "count",
            count.map(Value::Float).unwrap_or(Value::Null),
            "Update shopping count",
        ),
        LocalIntent::TouchLastSeenRemote { id, at } => set_field(
            id,
            "last_seen_remote",
            Value::String(at.clone()),
            "Mark shopping item seen",
        ),
        LocalIntent::Check { id } => {
            set_field(id, "checked", Value::Integer(1), "Check off shopping item")
        }
        LocalIntent::Delete { id } | LocalIntent::ReapTombstone { id } => {
            let mut params = std::collections::HashMap::new();
            params.insert("id".to_string(), Value::String(id.clone()));
            Operation::new(
                SHOPPING_ITEM_ENTITY,
                "delete",
                "Remove shopping item",
                params,
            )
        }
    }
}

fn set_field(id: &str, field: &str, value: Value, display: &str) -> Operation {
    let mut params = std::collections::HashMap::new();
    params.insert("id".to_string(), Value::String(id.to_string()));
    params.insert("field".to_string(), Value::String(field.to_string()));
    params.insert("value".to_string(), value);
    Operation::new(SHOPPING_ITEM_ENTITY, "set_field", display, params)
}

fn insert_params(row: &LocalShoppingItem) -> std::collections::HashMap<String, Value> {
    let mut params = std::collections::HashMap::new();
    params.insert("id".to_string(), Value::String(row.id.clone()));
    params.insert("name".to_string(), Value::String(row.name.clone()));
    params.insert(
        "cat".to_string(),
        Value::String(row.category.as_wire().to_string()),
    );
    params.insert(
        "count".to_string(),
        row.count.map(Value::Float).unwrap_or(Value::Null),
    );
    params.insert(
        "checked".to_string(),
        Value::Integer(i64::from(row.checked)),
    );
    params.insert(
        "product_id".to_string(),
        row.product_id
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    params.insert(
        "deleted_at".to_string(),
        row.deleted_at
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    params.insert(
        "last_seen_remote".to_string(),
        row.last_seen_remote
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    params
}
