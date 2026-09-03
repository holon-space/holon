//! Degraded-state bus for surfacing degradation to frontends (snapshot
//! save/load failures, rehydration errors, dead integrations).
//!
//! Unrelated to the block write path. Frontends subscribe via
//! [`DegradedSignalBus::subscribe`] and render banners; the subscription
//! carries the conditions already in effect plus a stream of later changes, so
//! a frontend that starts after a condition was raised still sees it.
//!
//! Producers emit and ignore lagged receivers — we prefer dropping
//! stale notifications over blocking the save worker.
//!
//! EVERY degradation is a sticky CONDITION: it is raised, stays in effect, and
//! is removed by [`DegradedSignalBus::clear`] at a named all-clear moment.
//! There is no transient-event class, because a transient emit is silently lost
//! whenever it wins the race against the subscriber — and the emitters that
//! race hardest (boot DI, the detached `post_ready` org scan) are exactly the
//! ones whose failures matter most. Each variant of [`ShareDegradedReason`]
//! documents its all-clear; a variant that cannot name one does not belong on
//! this bus.

use tokio::sync::broadcast;

/// Why a share is in a degraded state.
#[derive(Clone, Debug)]
pub enum ShareDegradedReason {
    /// Writing `<shared_tree_id>.loro` failed. The in-memory doc still
    /// holds the edit; the next commit will retry. String carries the
    /// underlying error.
    ///
    /// All-clear: the next successful save of the same share.
    SnapshotSaveFailed(String),
    /// Reading `<shared_tree_id>.loro` failed at startup. The file has
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
    /// `reason` carries the adapter's error. `shared_tree_id` is the file —
    /// one condition per bad file, so a repaired file lifts its own banner and
    /// leaves the others standing.
    ///
    /// All-clear: the next fully-successful ingest of that same file, emitted
    /// by `FileSyncController` through its `WritebackDisclosure` seam.
    VaultIngestFailed { format: String, reason: String },
    /// A block inside a shared subtree was edited, but its content could NOT be
    /// materialized to a dedicated on-disk org file (the mount is not yet a
    /// page-file, so the write-back layer cannot resolve a path). The edit is
    /// safe in Loro + SQL and syncs to peers, but disk org is stale until
    /// materialization is wired. Disclosed (not silently dropped) so the gap is
    /// visible. String carries the offending block id. `shared_tree_id` names
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
    /// the refusing adapter's own name; `shared_tree_id` is the file, so one
    /// condition stands per authoritative file.
    ///
    /// All-clear: none. The file is read-only for as long as its format is, so
    /// the condition is true for the session.
    EditRefusedReadOnlyFormat { format: String },
    /// The org write-back stream died and its supervisor could not keep it
    /// alive — edits reach Loro + SQL but stop reaching disk. String carries
    /// the supervisor's escalation summary (what died, how often).
    /// `shared_tree_id` is the sentinel `"org-writeback"`.
    ///
    /// All-clear: a successful stream respawn. No emitter of either half yet —
    /// the let-it-die supervisor owns both.
    WritebackDegraded(String),
    /// An MCP integration provider did not come up at boot — its sidecar
    /// command is missing/dead, or its `${VAR}` credentials are unresolved. The
    /// integration's `cc_*` cache tables are never created, so every page that
    /// queries them renders blank; disclosed so that blankness is attributable
    /// instead of looking like a healthy empty result. `shared_tree_id` carries
    /// the integration name (this is not tied to a shared doc).
    ///
    /// All-clear: the provider connecting.
    IntegrationConnectFailed { integration: String, error: String },
    /// An MCP integration provider needs an OAuth grant before it can connect.
    /// Same blank-page consequence as `IntegrationConnectFailed`, but the fix
    /// is a user action, so it carries the authorization URL.
    /// `shared_tree_id` carries the integration name.
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
    /// `schema_version`) needs no guessing. `shared_tree_id` carries the
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
    /// the log, which has room for it. `shared_tree_id` carries the integration
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
    /// An installed sidecar names a provider this build does not ship. Presence
    /// is settled at compile time, so nothing on disk can introduce a provider
    /// and the file does nothing at all. `shared_tree_id` carries the file
    /// stem.
    ///
    /// All-clear: none — the build would have to ship the provider.
    IntegrationSidecarNotBundled {
        provider: String,
        installed_path: String,
    },
}

impl ShareDegradedReason {
    /// Kind constants, so an all-clear site names the condition it lifts
    /// through the compiler instead of retyping the string.
    pub const FOREIGN_ID_COLLISION: &'static str = "foreign-id-collision";
    pub const INTEGRATION_CONNECT_FAILED: &'static str = "integration-connect-failed";
    pub const INTEGRATION_NEEDS_AUTH: &'static str = "integration-needs-auth";
    pub const INTEGRATION_NOT_ENABLED: &'static str = "integration-not-enabled";
    pub const INTEGRATION_SIDECAR_NOT_BUNDLED: &'static str = "integration-sidecar-not-bundled";
    pub const INTEGRATION_SIDECAR_SUPERSEDED: &'static str = "integration-sidecar-superseded";
    pub const REHYDRATION_FAILED: &'static str = "rehydration-failed";
    pub const SHARED_SUBTREE_NOT_MATERIALIZED: &'static str = "shared-subtree-not-materialized";
    pub const SNAPSHOT_LOAD_FAILED: &'static str = "snapshot-load-failed";
    pub const SNAPSHOT_SAVE_FAILED: &'static str = "snapshot-save-failed";
    pub const SQL_PROJECTION_FAILED: &'static str = "sql-projection-failed";
    pub const VAULT_INGEST_FAILED: &'static str = "vault-ingest-failed";
    pub const WRITEBACK_DEGRADED: &'static str = "writeback-degraded";
    pub const EDIT_REFUSED_READ_ONLY_FORMAT: &'static str = "edit-refused-read-only-format";

    /// The condition's stable identity, paired with the subject to form a
    /// [`DegradedConditionKey`]. Total: every degradation is a sticky
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
            Self::SnapshotSaveFailed(_) => Self::SNAPSHOT_SAVE_FAILED,
            Self::SnapshotLoadFailed(_) => Self::SNAPSHOT_LOAD_FAILED,
            Self::RehydrationFailed(_) => Self::REHYDRATION_FAILED,
            Self::SqlProjectionFailed(_) => Self::SQL_PROJECTION_FAILED,
            Self::ForeignIdCollision(_) => Self::FOREIGN_ID_COLLISION,
            Self::VaultIngestFailed { .. } => Self::VAULT_INGEST_FAILED,
            Self::SharedSubtreeNotMaterialized { .. } => Self::SHARED_SUBTREE_NOT_MATERIALIZED,
            Self::WritebackDegraded(_) => Self::WRITEBACK_DEGRADED,
            Self::EditRefusedReadOnlyFormat { .. } => Self::EDIT_REFUSED_READ_ONLY_FORMAT,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ShareDegraded {
    pub shared_tree_id: String,
    pub reason: ShareDegradedReason,
}

impl ShareDegraded {
    /// The sticky identity of this degradation.
    pub fn condition_key(&self) -> DegradedConditionKey {
        DegradedConditionKey {
            subject: self.shared_tree_id.clone(),
            kind: self.reason.condition_kind(),
        }
    }
}

/// Identity of a sticky degraded condition. `subject` is the
/// `shared_tree_id` — for integrations, the integration name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DegradedConditionKey {
    pub subject: String,
    pub kind: &'static str,
}

/// A change to the degraded state.
#[derive(Clone, Debug)]
pub enum DegradedChange {
    Raised(ShareDegraded),
    Cleared(DegradedConditionKey),
}

impl DegradedChange {
    /// The raised event, or `None` when this change is a clear.
    pub fn raised(self) -> Option<ShareDegraded> {
        match self {
            Self::Raised(event) => Some(event),
            Self::Cleared(_) => None,
        }
    }
}

/// What a subscriber gets: the degraded conditions that are currently in
/// effect, plus the stream of subsequent changes.
pub struct DegradedSubscription {
    pub current: Vec<ShareDegraded>,
    pub changes: broadcast::Receiver<DegradedChange>,
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
pub struct DegradedSignalBus {
    tx: broadcast::Sender<DegradedChange>,
    /// Insertion order makes the replay in `subscribe` deterministic. N is
    /// bounded by degraded subjects times kinds — single digits in practice —
    /// so linear search beats a map.
    conditions: std::sync::Mutex<Vec<ShareDegraded>>,
}

impl DegradedSignalBus {
    /// Channel capacity. Chosen to absorb a short burst of failures
    /// (e.g., transient filesystem permission error on several shares
    /// at once) without any slow subscriber losing them.
    const CAPACITY: usize = 64;

    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(Self::CAPACITY);
        Self {
            tx,
            conditions: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Raise a condition: recorded as current state (replacing any prior entry
    /// with the same key) and broadcast.
    pub fn emit(&self, event: ShareDegraded) {
        let key = event.condition_key();
        {
            let mut conditions = self.conditions.lock().unwrap();
            match conditions.iter_mut().find(|c| c.condition_key() == key) {
                Some(existing) => *existing = event.clone(),
                None => conditions.push(event.clone()),
            }
        }
        let _ = self.tx.send(DegradedChange::Raised(event));
    }

    /// Clear a condition. Broadcasts only if the condition was actually in
    /// effect, so consumers never see a phantom clear.
    pub fn clear(&self, key: &DegradedConditionKey) {
        let mut conditions = self.conditions.lock().unwrap();
        let before = conditions.len();
        conditions.retain(|c| &c.condition_key() != key);
        let removed = conditions.len() != before;
        drop(conditions);
        if removed {
            let _ = self.tx.send(DegradedChange::Cleared(key.clone()));
        }
    }

    /// Live subscriber count. A bus that conditions are raised on with zero
    /// subscribers discloses to nobody — the frontend wiring is then mute even
    /// though every producer looks correctly wired, so this is the observable
    /// that makes "someone is listening" assertable.
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }

    pub fn subscribe(&self) -> DegradedSubscription {
        // Hold the conditions lock across `tx.subscribe()` so snapshot and
        // subscription are atomic against `emit`, which takes the same lock:
        // subscribing first can at worst deliver a condition twice (consumers
        // upsert by key), whereas snapshotting first could lose one entirely.
        let conditions = self.conditions.lock().unwrap();
        let changes = self.tx.subscribe();
        let current = conditions.clone();
        drop(conditions);
        DegradedSubscription { current, changes }
    }
}

impl Default for DegradedSignalBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raised(change: DegradedChange) -> ShareDegraded {
        change.raised().expect("expected Raised")
    }

    fn connect_failed(name: &str) -> ShareDegraded {
        ShareDegraded {
            shared_tree_id: name.into(),
            reason: ShareDegradedReason::IntegrationConnectFailed {
                integration: name.into(),
                error: "sidecar died".into(),
            },
        }
    }

    /// Every condition names an all-clear, and clearing it works uniformly —
    /// stickiness without a clearing path is a permanent banner.
    #[tokio::test(flavor = "current_thread")]
    async fn a_share_condition_clears_by_its_kind() {
        let bus = DegradedSignalBus::new();
        bus.emit(ShareDegraded {
            shared_tree_id: "s".into(),
            reason: ShareDegradedReason::SnapshotSaveFailed("disk full".into()),
        });
        bus.clear(&DegradedConditionKey {
            subject: "s".into(),
            kind: ShareDegradedReason::SNAPSHOT_SAVE_FAILED,
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
        let bus = DegradedSignalBus::new();
        bus.emit(ShareDegraded {
            shared_tree_id: "s".into(),
            reason: ShareDegradedReason::SnapshotSaveFailed("disk full".into()),
        });
        bus.emit(ShareDegraded {
            shared_tree_id: "s".into(),
            reason: ShareDegradedReason::SqlProjectionFailed("table locked".into()),
        });
        assert_eq!(bus.subscribe().current.len(), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn subscriber_receives_event() {
        let bus = DegradedSignalBus::new();
        let mut sub = bus.subscribe();
        bus.emit(ShareDegraded {
            shared_tree_id: "abc".into(),
            reason: ShareDegradedReason::SnapshotLoadFailed("/tmp/x.corrupt-1".into()),
        });
        let ev = raised(sub.changes.recv().await.unwrap());
        assert_eq!(ev.shared_tree_id, "abc");
        assert!(matches!(
            ev.reason,
            ShareDegradedReason::SnapshotLoadFailed(ref p) if p.contains("corrupt")
        ));
    }

    /// The boot seam: integration failures are raised inside boot DI, before
    /// the window (the only consumer) exists.
    #[tokio::test(flavor = "current_thread")]
    async fn boot_time_conditions_reach_a_later_subscriber() {
        let bus = DegradedSignalBus::new();
        bus.emit(connect_failed("github"));
        bus.emit(ShareDegraded {
            shared_tree_id: "linear".into(),
            reason: ShareDegradedReason::IntegrationNeedsAuth {
                integration: "linear".into(),
                auth_url: "https://linear.app/oauth".into(),
            },
        });

        // window launches, strictly later
        let sub = bus.subscribe();

        let learned: Vec<String> = sub
            .current
            .iter()
            .map(|e| e.shared_tree_id.clone())
            .collect();
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
            ShareDegradedReason::VaultIngestFailed {
                format: "org".into(),
                reason: "notes.org: unparseable".into(),
            },
            ShareDegradedReason::SnapshotSaveFailed("disk full".into()),
            ShareDegradedReason::SnapshotLoadFailed("/v/s.loro.corrupt-1".into()),
            ShareDegradedReason::RehydrationFailed("advertiser: port in use".into()),
            ShareDegradedReason::SqlProjectionFailed("table locked".into()),
            ShareDegradedReason::ForeignIdCollision("block:journals".into()),
            ShareDegradedReason::SharedSubtreeNotMaterialized {
                file: "/vault/Projects/Shared.org".into(),
            },
            ShareDegradedReason::WritebackDegraded("stream died 3x".into()),
        ];

        for reason in raised_before_anyone_listens {
            let bus = DegradedSignalBus::new();
            bus.emit(ShareDegraded {
                shared_tree_id: "subject".into(),
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
        let bus = DegradedSignalBus::new();
        bus.emit(connect_failed("github"));
        bus.emit(ShareDegraded {
            shared_tree_id: "github".into(),
            reason: ShareDegradedReason::IntegrationConnectFailed {
                integration: "github".into(),
                error: "still dead".into(),
            },
        });
        let sub = bus.subscribe();
        assert_eq!(sub.current.len(), 1);
        assert!(matches!(
            sub.current[0].reason,
            ShareDegradedReason::IntegrationConnectFailed { ref error, .. } if error == "still dead"
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn clear_removes_the_condition_and_notifies() {
        let bus = DegradedSignalBus::new();
        bus.emit(connect_failed("github"));
        let mut sub = bus.subscribe();
        assert_eq!(sub.current.len(), 1);

        let key = DegradedConditionKey {
            subject: "github".into(),
            kind: "integration-connect-failed",
        };
        bus.clear(&key);

        assert!(matches!(
            sub.changes.recv().await.unwrap(),
            DegradedChange::Cleared(k) if k == key
        ));
        assert!(bus.subscribe().current.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn clearing_an_unraised_condition_broadcasts_nothing() {
        let bus = DegradedSignalBus::new();
        let mut sub = bus.subscribe();
        bus.clear(&DegradedConditionKey {
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
        let bus = DegradedSignalBus::new();
        let mut sub1 = bus.subscribe();
        let mut sub2 = bus.subscribe();
        bus.emit(ShareDegraded {
            shared_tree_id: "x".into(),
            reason: ShareDegradedReason::RehydrationFailed("endpoint".into()),
        });
        assert_eq!(
            raised(sub1.changes.recv().await.unwrap()).shared_tree_id,
            "x"
        );
        assert_eq!(
            raised(sub2.changes.recv().await.unwrap()).shared_tree_id,
            "x"
        );
    }
}
