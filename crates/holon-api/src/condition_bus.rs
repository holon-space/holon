//! Degraded-state bus for surfacing degradation to frontends (snapshot
//! save/load failures, rehydration errors, dead integrations).
//!
//! Unrelated to the block write path. Frontends subscribe via
//! [`ConditionBus::subscribe`] and render banners; the subscription
//! carries the conditions already in effect plus a stream of later changes, so
//! a frontend that starts after a condition was raised still sees it.
//!
//! Producers emit and ignore lagged receivers — we prefer dropping
//! stale notifications over blocking the save worker.
//!
//! EVERY degradation is a sticky CONDITION: it is raised, stays in effect, and
//! is removed by [`ConditionBus::clear`] at a named all-clear moment.
//! There is no transient-event class, because a transient emit is silently lost
//! whenever it wins the race against the subscriber — and the emitters that
//! race hardest (boot DI, the detached `post_ready` org scan) are exactly the
//! ones whose failures matter most. Each variant of [`ConditionKind`]
//! documents its all-clear; a variant that cannot name one does not belong on
//! this bus.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::broadcast;

use crate::live_data::LiveData;

/// Why a share is in a degraded state.
#[derive(Clone, Debug)]
pub enum ConditionKind {
    /// Writing `<subject>.loro` failed. The in-memory doc still
    /// holds the edit; the next commit will retry. String carries the
    /// underlying error.
    ///
    /// All-clear: the next successful save of the same share.
    SnapshotSaveFailed(String),
    /// Reading `<subject>.loro` failed at startup. The file has
    /// been renamed to `<path>.corrupt-<ts>` (carried in the string).
    /// The share is **not** registered — peer must re-accept to recover.
    ///
    /// All-clear: sticky until the share is registered again, which only a
    /// re-accept can do. Load runs once per process, so in practice this
    /// condition lives until restart — and it should: the share really is
    /// missing for the whole session.
    SnapshotLoadFailed(String),
    /// Rehydration encountered an error after `load` succeeded — most
    /// commonly an advertiser-start failure on a non-idempotent code
    /// path. String carries the underlying error.
    ///
    /// All-clear: sticky until the share rehydrates successfully. Rehydration
    /// runs once at startup and has no retry loop, so this holds until restart.
    RehydrationFailed(String),
    /// Projecting a shared doc's change into the SQL `block` table failed.
    /// Loro holds the change but SQL (which the UI reads) does not, so the
    /// two diverge until the next successful projection. The projection
    /// watermark is deliberately NOT advanced on failure, so the next
    /// commit retries the same diff. String carries the underlying error.
    ///
    /// All-clear: the next successful projection of the same share.
    SqlProjectionFailed(String),
    /// A shared doc tried to project a block whose id collides with a LIVE
    /// node in the recipient's global tree — i.e. it is trying to shadow a
    /// LOCAL block id (e.g. a malicious sharer naming a node `block:journals`).
    /// The projection is refused so it cannot clobber the recipient's own SQL
    /// row; the watermark is NOT advanced, so an honest later diff still
    /// projects. String carries the colliding block id.
    ///
    /// All-clear: the next successful projection of the same share — that is
    /// the moment an honest diff got through, so the refusal no longer holds.
    ForeignIdCollision(String),
    /// One vault file was REFUSED by its format adapter, so nothing of it is
    /// in the store. The app stays up and the OTHER files keep syncing (the
    /// watch loop is armed), but this file is NOT ingested until it is fixed —
    /// a visible degraded mode, not a silent sync death.
    ///
    /// `format` is the refusing adapter's own name (`org`, `cooklang`, …), so
    /// the banner sends the reader to the defect the file can actually have;
    /// `reason` carries the adapter's error. `subject` is the file —
    /// one condition per bad file, so a repaired file lifts its own banner and
    /// leaves the others standing.
    ///
    /// All-clear: the next fully-successful ingest of that same file, emitted
    /// by `FileSyncController` through its `WritebackDisclosure` seam.
    VaultIngestFailed { format: String, reason: String },
    /// One vault file is 0 bytes and stayed that way past the grace period that
    /// covers an atomic save's zero-length intermediate — a file someone really
    /// emptied. Nothing of it is ingested, because an empty file cannot
    /// identify the document that lives at its path, so the document Holon
    /// already holds is KEPT: the store shows content the file no longer has.
    /// Disclosed rather than converged either way — deleting on an empty file's
    /// word loses content, and writing the document back would undo the user's
    /// own edit. `subject` is the file.
    ///
    /// All-clear: the next successful ingest of that same file, i.e. the moment
    /// it has content again (or is deleted, which the watcher handles as a
    /// deletion).
    VaultFileEmptied,
    /// A block inside a shared subtree was edited, but its content could NOT be
    /// materialized to a dedicated on-disk org file (the mount is not yet a
    /// page-file, so the write-back layer cannot resolve a path). The edit is
    /// safe in Loro + SQL and syncs to peers, but disk org is stale until
    /// materialization is wired. Disclosed (not silently dropped) so the gap is
    /// visible. String carries the offending block id. `subject` names
    /// the share.
    ///
    /// All-clear: the first successful org materialization of that share's
    /// mount. Materialization is not built yet, so nothing can raise the
    /// all-clear and the condition holds for the session — accurately, since
    /// the disk projection stays stale for exactly that long.
    ///
    /// `file` is the document the shared content was inlined into — the thing
    /// a user can open. Typed rather than pre-formatted so the frontend
    /// decides how to present it.
    SharedSubtreeNotMaterialized { file: String },
    /// An edit named a block whose file's format Holon cannot write, so the
    /// operation dispatcher refused it and the store never took it. `format` is
    /// the refusing adapter's own name; `subject` is the file, so one
    /// condition stands per authoritative file.
    ///
    /// All-clear: none. The file is read-only for as long as its format is, so
    /// the condition is true for the session.
    EditRefusedReadOnlyFormat { format: String },
    /// A typed edit never reached the store because the editor could not take
    /// the document's write lock. `subject` is the block being typed into, and
    /// `detail` is the lock's own account of why. The keystroke is lost: the
    /// editor holds text the store does not.
    ///
    /// All-clear: none. Nothing re-applies the lost keystroke, so the notice
    /// stands until the user retypes and the session restarts.
    LocalEditNotApplied { detail: String },
    /// The org write-back stream died and its supervisor could not keep it
    /// alive — edits reach Loro + SQL but stop reaching disk. String carries
    /// the supervisor's escalation summary (what died, how often).
    /// `subject` is the sentinel `"org-writeback"`.
    ///
    /// All-clear: a successful stream respawn. No emitter of either half yet —
    /// the let-it-die supervisor owns both.
    WritebackDegraded(String),
    /// An MCP integration provider did not come up at boot — its sidecar
    /// command is missing/dead, or its `${VAR}` credentials are unresolved. The
    /// integration's `cc_*` cache tables are never created, so every page that
    /// queries them renders blank; disclosed so that blankness is attributable
    /// instead of looking like a healthy empty result. `subject` carries
    /// the integration name (this is not tied to a shared doc).
    ///
    /// All-clear: the provider connecting.
    IntegrationConnectFailed { integration: String, error: String },
    /// An MCP integration provider needs an OAuth grant before it can connect.
    /// Same blank-page consequence as `IntegrationConnectFailed`, but the fix
    /// is a user action, so it carries the authorization URL.
    /// `subject` carries the integration name.
    ///
    /// All-clear: the grant completing, i.e. the provider connecting.
    IntegrationNeedsAuth {
        integration: String,
        auth_url: String,
    },
    /// An INSTALLED sidecar for a provider this build ships could not be
    /// honored, so the bundled sidecar was used instead. The integration works;
    /// what is degraded is the user's expectation that the file they installed
    /// is what runs. Disclosed with both paths and the incompatibility so the
    /// remedy (delete the file, or re-author it against this build's
    /// `schema_version`) needs no guessing. `subject` carries the
    /// integration name.
    ///
    /// All-clear: none within a session — the choice is made once at boot. The
    /// condition ends when the installed file is fixed and the app restarts.
    IntegrationSidecarSuperseded {
        integration: String,
        installed_path: String,
        bundled_source: String,
        incompatibility: String,
    },
    /// An installed sidecar for a provider this build ships enabled NOTHING,
    /// because enablement is the integration state's decision and that state
    /// does not say `enabled`. Before the state store existed the file itself
    /// was the switch, so this is the shape a pre-cutover setup arrives in: the
    /// user believes the integration is on and every page it feeds is blank.
    /// Carries the state file to write; the full content to put in it goes to
    /// the log, which has room for it. `subject` carries the integration
    /// name.
    ///
    /// All-clear: none within a session — enablement is read once at boot. The
    /// condition ends when the state file is written and the app restarts.
    IntegrationNotEnabled {
        integration: String,
        installed_path: String,
        state_path: String,
        /// The command that switches it on, composed by the loader so the UI
        /// renders one instruction that works instead of inventing its own.
        remedy: String,
    },
    /// A state file names a connection nothing provides — the build ships no
    /// sidecar for it and no usable file introduces one. A leftover state file
    /// after a rename or a deletion is the usual cause. `subject`
    /// carries the file stem.
    ///
    /// All-clear: none — a connection of that name has to start existing.
    IntegrationSidecarNotBundled {
        provider: String,
        installed_path: String,
    },
    /// This session keeps every secret in RAM and loses it on exit
    /// (`HOLON_SECRETS_BACKEND=memory`, a fixture mode). A credential field the
    /// user fills in saves nothing, which is the one thing about it they must
    /// not have to infer. `subject` is the fixed subject `secrets`.
    ///
    /// All-clear: none within a session — the backend is chosen once at boot.
    /// The condition ends when the process restarts without that variable.
    ///
    /// `seed_path` is the fixture file this session was pre-loaded from, when
    /// it was. It travels beside the sentence rather than inside it because the
    /// toast caps the sentence and a path cut in half names nothing.
    SecretsHeldInMemory {
        why: String,
        seed_path: Option<String>,
    },
    /// The undo history from the previous session was discarded at boot
    /// (D116.a): undo deliberately does NOT survive a restart, because the
    /// CRDT's text-undo manager is rebuilt empty and a journal entry standing
    /// for typing it no longer holds could not be honoured. Cleared rather
    /// than half-kept, and disclosed rather than silently absent — a person
    /// who typed before the restart would otherwise press cmd-z and get
    /// nothing with no explanation.
    ///
    /// All-clear: none — it is a one-shot notice about this boot.
    UndoHistoryClearedAtBoot { entries: usize },
    /// A file NAMES a connection but cannot be used, so the connection does
    /// not exist: a stale `schema_version`, a file that will not parse, a
    /// reference to another connection's secret, or two files claiming one
    /// name. Distinct from the case above, where nothing named it at all —
    /// here the remedy is to fix the file the message points at, not to add a
    /// provider. `subject` carries the file stem.
    ///
    /// All-clear: none — the file has to change.
    IntegrationSidecarUnusable {
        provider: String,
        installed_path: String,
        why: String,
    },
    /// This device was paired to an owner AFTER it had been used on its own, so
    /// the store it now shows is the owner's plus the `blocks` it wrote here,
    /// re-imported under their own ids. Informational rather than a fault, and
    /// deliberately not a choice: the ruling (D78.d) is that content written on
    /// this device is kept. `conflict_copies` of those blocks held an id the
    /// owner also had, with different content, and are kept as a child of the
    /// owner's block. `archive` holds the pre-pair document and is the handle
    /// on anything the re-import could not carry. `subject` carries the
    /// pairing entity name.
    ///
    /// All-clear: none — a pair happens once and what it says stays true.
    PairingReimportedLocalContent {
        blocks: usize,
        conflict_copies: usize,
        archive: String,
    },
    /// `orphans` block(s) this device wrote before it was paired are NOT in the
    /// store it now shows: their parent is in neither the owner's store nor the
    /// archive, so the re-import has nowhere to put them. The app boots without
    /// them rather than not at all — retrying that re-import at every boot
    /// would refuse identically until the parent appears.
    ///
    /// `archive` is where those blocks still are, and is the only copy. The
    /// pairing marker is kept beside it, so the next boot and the
    /// `device.pair_retry_reimport` action both re-attempt the same work.
    ///
    /// All-clear: a re-import that completes, which is the moment the archive's
    /// content reaches the store.
    PairingReimportDeferred { orphans: usize, archive: String },
    /// A peer joined this share by proving the ticket's BEARER capability:
    /// possession of the ticket string is the whole credential, so whoever the
    /// ticket was forwarded to could have joined instead. That is the stopgap
    /// ADR 0028 R5 accepts until enrollment binds a device key — disclosed
    /// here so the user can see that this share's trust rests on a secret that
    /// travelled, not on a paired identity. `peer` is the QUIC-authenticated
    /// node key (public); the capability secret is never carried.
    ///
    /// All-clear: never automatic — it is a fact about how the share was
    /// joined, and it ends only when the share is unshared or the peer
    /// revoked.
    BearerTicketEnrollment { peer: String },
    /// This device minted its owner identity key on its first share, and the
    /// one-time recovery code that key produced could not be shown to anyone —
    /// no surface exists yet that can display it (ADR 0028 D1 defers the mint
    /// to first use, and the code is show-once by construction).
    ///
    /// The consequence is bounded and worth stating exactly: the shared DATA is
    /// not at risk, and neither is any peer's access. What is lost with the
    /// keychain entry is this device's ability to sign and verify its own
    /// roster sidecars — after which its shares fail CLOSED (they are not
    /// advertised) rather than open.
    ///
    /// The code itself is never carried here, logged, or rendered; this
    /// condition says only that one existed and went unshown. `subject`
    /// is the sentinel [`OWNER_IDENTITY_SUBJECT`], because the key is
    /// device-wide rather than per-share.
    ///
    /// All-clear: none — it is a fact about a mint that already happened. It
    /// ends when a recovery-code surface exists and the user has seen a
    /// freshly rotated code.
    OwnerRecoveryCodeNotShown,
}

/// Subject of the device-wide conditions on this bus, which have no share to
/// name. Used by [`ConditionKind::OwnerRecoveryCodeNotShown`].
pub const OWNER_IDENTITY_SUBJECT: &str = "owner-identity";

impl ConditionKind {
    /// Kind constants, so an all-clear site names the condition it lifts
    /// through the compiler instead of retyping the string.
    pub const FOREIGN_ID_COLLISION: &'static str = "foreign-id-collision";
    pub const INTEGRATION_CONNECT_FAILED: &'static str = "integration-connect-failed";
    pub const INTEGRATION_NEEDS_AUTH: &'static str = "integration-needs-auth";
    pub const INTEGRATION_NOT_ENABLED: &'static str = "integration-not-enabled";
    pub const INTEGRATION_SIDECAR_NOT_BUNDLED: &'static str = "integration-sidecar-not-bundled";
    pub const INTEGRATION_SIDECAR_UNUSABLE: &'static str = "integration-sidecar-unusable";
    pub const INTEGRATION_SIDECAR_SUPERSEDED: &'static str = "integration-sidecar-superseded";
    pub const PAIRING_REIMPORTED_LOCAL_CONTENT: &'static str = "pairing-reimported-local-content";
    pub const PAIRING_REIMPORT_DEFERRED: &'static str = "pairing-reimport-deferred";
    pub const REHYDRATION_FAILED: &'static str = "rehydration-failed";
    pub const SECRETS_HELD_IN_MEMORY: &'static str = "secrets-held-in-memory";
    pub const UNDO_HISTORY_CLEARED_AT_BOOT: &'static str = "undo-history-cleared-at-boot";
    pub const SHARED_SUBTREE_NOT_MATERIALIZED: &'static str = "shared-subtree-not-materialized";
    pub const SNAPSHOT_LOAD_FAILED: &'static str = "snapshot-load-failed";
    pub const SNAPSHOT_SAVE_FAILED: &'static str = "snapshot-save-failed";
    pub const SQL_PROJECTION_FAILED: &'static str = "sql-projection-failed";
    pub const VAULT_INGEST_FAILED: &'static str = "vault-ingest-failed";
    pub const VAULT_FILE_EMPTIED: &'static str = "vault-file-emptied";
    pub const WRITEBACK_DEGRADED: &'static str = "writeback-degraded";
    pub const EDIT_REFUSED_READ_ONLY_FORMAT: &'static str = "edit-refused-read-only-format";
    pub const LOCAL_EDIT_NOT_APPLIED: &'static str = "local-edit-not-applied";
    pub const BEARER_TICKET_ENROLLMENT: &'static str = "bearer-ticket-enrollment";
    pub const OWNER_RECOVERY_CODE_NOT_SHOWN: &'static str = "owner-recovery-code-not-shown";

    /// The condition's stable identity, paired with the subject to form a
    /// [`ConditionKey`]. Total: every degradation is a sticky
    /// condition, so a new variant cannot opt out of replay by accident — it
    /// can only fail to compile until it names its kind (and, per this enum's
    /// doc contract, its all-clear).
    pub fn condition_kind(&self) -> &'static str {
        match self {
            Self::IntegrationConnectFailed { .. } => Self::INTEGRATION_CONNECT_FAILED,
            Self::IntegrationNeedsAuth { .. } => Self::INTEGRATION_NEEDS_AUTH,
            Self::IntegrationSidecarSuperseded { .. } => Self::INTEGRATION_SIDECAR_SUPERSEDED,
            Self::IntegrationNotEnabled { .. } => Self::INTEGRATION_NOT_ENABLED,
            Self::IntegrationSidecarNotBundled { .. } => Self::INTEGRATION_SIDECAR_NOT_BUNDLED,
            Self::IntegrationSidecarUnusable { .. } => Self::INTEGRATION_SIDECAR_UNUSABLE,
            Self::SecretsHeldInMemory { .. } => Self::SECRETS_HELD_IN_MEMORY,
            Self::UndoHistoryClearedAtBoot { .. } => Self::UNDO_HISTORY_CLEARED_AT_BOOT,
            Self::SnapshotSaveFailed(_) => Self::SNAPSHOT_SAVE_FAILED,
            Self::SnapshotLoadFailed(_) => Self::SNAPSHOT_LOAD_FAILED,
            Self::RehydrationFailed(_) => Self::REHYDRATION_FAILED,
            Self::SqlProjectionFailed(_) => Self::SQL_PROJECTION_FAILED,
            Self::ForeignIdCollision(_) => Self::FOREIGN_ID_COLLISION,
            Self::VaultIngestFailed { .. } => Self::VAULT_INGEST_FAILED,
            Self::VaultFileEmptied => Self::VAULT_FILE_EMPTIED,
            Self::SharedSubtreeNotMaterialized { .. } => Self::SHARED_SUBTREE_NOT_MATERIALIZED,
            Self::WritebackDegraded(_) => Self::WRITEBACK_DEGRADED,
            Self::PairingReimportedLocalContent { .. } => Self::PAIRING_REIMPORTED_LOCAL_CONTENT,
            Self::PairingReimportDeferred { .. } => Self::PAIRING_REIMPORT_DEFERRED,
            Self::EditRefusedReadOnlyFormat { .. } => Self::EDIT_REFUSED_READ_ONLY_FORMAT,
            Self::LocalEditNotApplied { .. } => Self::LOCAL_EDIT_NOT_APPLIED,
            Self::BearerTicketEnrollment { .. } => Self::BEARER_TICKET_ENROLLMENT,
            Self::OwnerRecoveryCodeNotShown => Self::OWNER_RECOVERY_CODE_NOT_SHOWN,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Condition {
    pub subject: String,
    pub reason: ConditionKind,
}

impl Condition {
    /// The sticky identity of this degradation.
    pub fn condition_key(&self) -> ConditionKey {
        ConditionKey {
            subject: self.subject.clone(),
            kind: self.reason.condition_kind(),
        }
    }
}

/// Identity of a sticky degraded condition. `subject` is what the condition is
/// about — for integrations, the integration name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConditionKey {
    pub subject: String,
    pub kind: &'static str,
}

impl ConditionKey {
    /// The key this condition occupies in the bus's mirror.
    ///
    /// Subject FIRST, so the mirror's key order groups a subject's conditions
    /// together: `entries_signal_vec` emits in key order, which is how the
    /// per-subject view of the `conditions` row source comes out contiguous
    /// without a second, accumulating mirror.
    pub fn mirror_key(&self) -> String {
        format!("{}\u{1F}{}", self.subject, self.kind)
    }
}

/// A change to the degraded state.
#[derive(Clone, Debug)]
pub enum ConditionChange {
    Raised(Condition),
    Cleared(ConditionKey),
}

impl ConditionChange {
    /// The raised event, or `None` when this change is a clear.
    pub fn raised(self) -> Option<Condition> {
        match self {
            Self::Raised(event) => Some(event),
            Self::Cleared(_) => None,
        }
    }
}

/// What a subscriber gets: the degraded conditions that are currently in
/// effect, plus the stream of subsequent changes.
pub struct ConditionSubscription {
    pub current: Vec<Condition>,
    pub changes: broadcast::Receiver<ConditionChange>,
}

/// Degraded-state bus: a sticky map of the currently-raised conditions plus a
/// broadcast of changes to it.
///
/// Stickiness removes the ordering contract between emitters and subscribers —
/// a condition raised during boot DI still reaches a window that launches
/// afterwards.
///
/// Senders never block. Slow subscribers get `RecvError::Lagged` on
/// their next `recv()` and must catch up — they do not stall producers.
pub struct ConditionBus {
    tx: broadcast::Sender<ConditionChange>,
    /// The conditions in effect, as the sanctioned holder rather than a
    /// private `Vec`. Being a [`LiveData`] is what lets a collection widget
    /// render the set directly (the `conditions` row source) instead of every
    /// frontend keeping its own mirror of it — which is what let a glyph and a
    /// banner disagree. Nothing in it touches storage, so the bus is the
    /// authority in every configuration.
    conditions: Arc<LiveData<Condition>>,
    /// When each condition in effect was first raised.
    ///
    /// The holder is keyed by subject-then-kind, so ITS order is alphabetical,
    /// not chronological. Replay order is what a reader sees as toast stack
    /// order, and a stack that reshuffles itself by subject name when a window
    /// opens is not the order anything happened in — so the raise order is
    /// kept here and [`subscribe`](Self::subscribe) sorts by it.
    ///
    /// Re-raising an existing condition KEEPS its original position, matching
    /// the in-place replacement this bus has always done: a repeated failure
    /// must not make its toast jump the queue.
    raised_at: std::sync::Mutex<HashMap<String, u64>>,
    next_raise: std::sync::atomic::AtomicU64,
}

impl ConditionBus {
    /// Channel capacity. Chosen to absorb a short burst of failures
    /// (e.g., transient filesystem permission error on several shares
    /// at once) without any slow subscriber losing them.
    const CAPACITY: usize = 64;

    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(Self::CAPACITY);
        Self {
            tx,
            conditions: LiveData::in_memory(),
            raised_at: std::sync::Mutex::new(HashMap::new()),
            next_raise: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// The conditions in effect, as the live holder. This is what the
    /// `conditions` row source is registered over, so a rendered collection
    /// and a bus subscriber read the same set by construction.
    pub fn conditions(&self) -> Arc<LiveData<Condition>> {
        Arc::clone(&self.conditions)
    }

    /// Raise a condition: recorded as current state (replacing any prior entry
    /// with the same key) and broadcast.
    pub fn emit(&self, event: Condition) {
        let key = event.condition_key();
        let mirror_key = key.mirror_key();
        {
            // `raised_at` before the holder, the same order `subscribe` takes
            // them in, so the two can never deadlock against each other — and
            // so a subscriber's snapshot cannot catch a condition that is in
            // the holder but not yet in the order.
            let mut raised_at = self.raised_at.lock().unwrap();
            raised_at.entry(mirror_key.clone()).or_insert_with(|| {
                self.next_raise
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            });
            self.conditions.insert(mirror_key, Arc::new(event.clone()));
        }
        let _ = self.tx.send(ConditionChange::Raised(event));
    }

    /// Clear a condition. Broadcasts only if the condition was actually in
    /// effect, so consumers never see a phantom clear.
    pub fn clear(&self, key: &ConditionKey) {
        let mirror_key = key.mirror_key();
        let removed = {
            let mut raised_at = self.raised_at.lock().unwrap();
            raised_at.remove(&mirror_key);
            self.conditions.remove(&mirror_key)
        };
        if removed {
            let _ = self.tx.send(ConditionChange::Cleared(key.clone()));
        }
    }

    /// Live subscriber count. A bus that conditions are raised on with zero
    /// subscribers discloses to nobody — the frontend wiring is then mute even
    /// though every producer looks correctly wired, so this is the observable
    /// that makes "someone is listening" assertable.
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// The conditions in effect, in the order they were RAISED.
    ///
    /// The same snapshot [`subscribe`](Self::subscribe) replays, without
    /// opening a broadcast receiver — for a reader that wants to look once
    /// (the MCP `conditions_list` tool) rather than follow along.
    pub fn current(&self) -> Vec<Condition> {
        let raised_at = self.raised_at.lock().unwrap();
        Self::in_raise_order(&self.conditions, &raised_at)
    }

    fn in_raise_order(
        conditions: &LiveData<Condition>,
        raised_at: &HashMap<String, u64>,
    ) -> Vec<Condition> {
        let mut current: Vec<(u64, Condition)> = conditions
            .read()
            .iter()
            .map(|(key, condition)| {
                let seq = raised_at.get(key).copied().unwrap_or(u64::MAX);
                (seq, (**condition).clone())
            })
            .collect();
        // The holder is keyed subject-first, so its own order is alphabetical;
        // replaying in that order would reshuffle a reader's toast stack by
        // subject name.
        current.sort_by_key(|(seq, _)| *seq);
        current.into_iter().map(|(_, c)| c).collect()
    }

    pub fn subscribe(&self) -> ConditionSubscription {
        // Hold `raised_at` across the snapshot AND `tx.subscribe()` so the two
        // are atomic against `emit`, which takes the same lock first:
        // subscribing first can at worst deliver a condition twice (consumers
        // upsert by key), whereas snapshotting first could lose one entirely.
        let raised_at = self.raised_at.lock().unwrap();
        let changes = self.tx.subscribe();
        let current = Self::in_raise_order(&self.conditions, &raised_at);
        drop(raised_at);
        ConditionSubscription { current, changes }
    }
}

impl Default for ConditionBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raised(change: ConditionChange) -> Condition {
        change.raised().expect("expected Raised")
    }

    fn connect_failed(name: &str) -> Condition {
        Condition {
            subject: name.into(),
            reason: ConditionKind::IntegrationConnectFailed {
                integration: name.into(),
                error: "sidecar died".into(),
            },
        }
    }

    /// Every condition names an all-clear, and clearing it works uniformly —
    /// stickiness without a clearing path is a permanent banner.
    #[tokio::test(flavor = "current_thread")]
    async fn a_share_condition_clears_by_its_kind() {
        let bus = ConditionBus::new();
        bus.emit(Condition {
            subject: "s".into(),
            reason: ConditionKind::SnapshotSaveFailed("disk full".into()),
        });
        bus.clear(&ConditionKey {
            subject: "s".into(),
            kind: ConditionKind::SNAPSHOT_SAVE_FAILED,
        });
        assert!(
            bus.subscribe().current.is_empty(),
            "the next successful save must leave no banner behind"
        );
    }

    /// Kinds are the sticky identity, so two different degradations of the
    /// same subject must not overwrite each other.
    #[tokio::test(flavor = "current_thread")]
    async fn distinct_kinds_on_one_subject_coexist() {
        let bus = ConditionBus::new();
        bus.emit(Condition {
            subject: "s".into(),
            reason: ConditionKind::SnapshotSaveFailed("disk full".into()),
        });
        bus.emit(Condition {
            subject: "s".into(),
            reason: ConditionKind::SqlProjectionFailed("table locked".into()),
        });
        assert_eq!(bus.subscribe().current.len(), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn subscriber_receives_event() {
        let bus = ConditionBus::new();
        let mut sub = bus.subscribe();
        bus.emit(Condition {
            subject: "abc".into(),
            reason: ConditionKind::SnapshotLoadFailed("/tmp/x.corrupt-1".into()),
        });
        let ev = raised(sub.changes.recv().await.unwrap());
        assert_eq!(ev.subject, "abc");
        assert!(matches!(
            ev.reason,
            ConditionKind::SnapshotLoadFailed(ref p) if p.contains("corrupt")
        ));
    }

    /// The boot seam: integration failures are raised inside boot DI, before
    /// the window (the only consumer) exists.
    #[tokio::test(flavor = "current_thread")]
    async fn boot_time_conditions_reach_a_later_subscriber() {
        let bus = ConditionBus::new();
        bus.emit(connect_failed("github"));
        bus.emit(Condition {
            subject: "linear".into(),
            reason: ConditionKind::IntegrationNeedsAuth {
                integration: "linear".into(),
                auth_url: "https://linear.app/oauth".into(),
            },
        });

        // window launches, strictly later
        let sub = bus.subscribe();

        let learned: Vec<String> = sub.current.iter().map(|e| e.subject.clone()).collect();
        assert_eq!(
            learned,
            vec!["github".to_string(), "linear".to_string()],
            "a subscriber that arrives after boot must still learn the current degraded conditions"
        );
    }

    /// The boot race the integration seam already fixed, for the OTHER
    /// emitters: `wiring.rs`'s detached `post_ready` task emits
    /// `VaultIngestFailed` while the window is still launching. A fast-failing
    /// initial scan lands before the disclosure bridge subscribes; without
    /// replay the banner is dropped and the vault silently half-syncs.
    #[tokio::test(flavor = "current_thread")]
    async fn every_degradation_reaches_a_subscriber_that_arrives_after_it_was_raised() {
        let raised_before_anyone_listens = vec![
            ConditionKind::VaultIngestFailed {
                format: "org".into(),
                reason: "notes.org: unparseable".into(),
            },
            ConditionKind::SnapshotSaveFailed("disk full".into()),
            ConditionKind::SnapshotLoadFailed("/v/s.loro.corrupt-1".into()),
            ConditionKind::RehydrationFailed("advertiser: port in use".into()),
            ConditionKind::SqlProjectionFailed("table locked".into()),
            ConditionKind::ForeignIdCollision("block:journals".into()),
            ConditionKind::SharedSubtreeNotMaterialized {
                file: "/vault/Projects/Shared.org".into(),
            },
            ConditionKind::WritebackDegraded("stream died 3x".into()),
        ];

        for reason in raised_before_anyone_listens {
            let bus = ConditionBus::new();
            bus.emit(Condition {
                subject: "subject".into(),
                reason: reason.clone(),
            });

            // The window (the only consumer) launches strictly later.
            let sub = bus.subscribe();

            assert_eq!(
                sub.current.len(),
                1,
                "a degradation raised before the disclosure bridge subscribed must still reach \
                 it — otherwise the app boots looking healthy while {reason:?} is in effect"
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn re_raising_a_condition_replaces_rather_than_accumulates() {
        let bus = ConditionBus::new();
        bus.emit(connect_failed("github"));
        bus.emit(Condition {
            subject: "github".into(),
            reason: ConditionKind::IntegrationConnectFailed {
                integration: "github".into(),
                error: "still dead".into(),
            },
        });
        let sub = bus.subscribe();
        assert_eq!(sub.current.len(), 1);
        assert!(matches!(
            sub.current[0].reason,
            ConditionKind::IntegrationConnectFailed { ref error, .. } if error == "still dead"
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn clear_removes_the_condition_and_notifies() {
        let bus = ConditionBus::new();
        bus.emit(connect_failed("github"));
        let mut sub = bus.subscribe();
        assert_eq!(sub.current.len(), 1);

        let key = ConditionKey {
            subject: "github".into(),
            kind: "integration-connect-failed",
        };
        bus.clear(&key);

        assert!(matches!(
            sub.changes.recv().await.unwrap(),
            ConditionChange::Cleared(k) if k == key
        ));
        assert!(bus.subscribe().current.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn clearing_an_unraised_condition_broadcasts_nothing() {
        let bus = ConditionBus::new();
        let mut sub = bus.subscribe();
        bus.clear(&ConditionKey {
            subject: "github".into(),
            kind: "integration-connect-failed",
        });
        assert!(matches!(
            sub.changes.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn multiple_subscribers_all_see_events() {
        let bus = ConditionBus::new();
        let mut sub1 = bus.subscribe();
        let mut sub2 = bus.subscribe();
        bus.emit(Condition {
            subject: "x".into(),
            reason: ConditionKind::RehydrationFailed("endpoint".into()),
        });
        assert_eq!(raised(sub1.changes.recv().await.unwrap()).subject, "x");
        assert_eq!(raised(sub2.changes.recv().await.unwrap()).subject, "x");
    }
}
