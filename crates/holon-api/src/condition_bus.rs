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

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::num::NonZeroUsize;
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
    /// Vault files were REFUSED by their format adapter, so nothing of them is
    /// in the store. The app stays up and the OTHER files keep syncing, but a
    /// refused file is NOT ingested until it is fixed.
    ///
    /// One condition per FORMAT (`org`, `cooklang`, …), never per file: a vault
    /// can refuse thousands of files of one format. `subject` is the format.
    /// The bus keeps the per-file record behind it
    /// ([`ConditionBus::vault_ingest_refused`]), so the count follows every
    /// refusal and repair, and [`ConditionBus::refused_files`] lists each file
    /// with its reason.
    ///
    /// All-clear: the last refused file of that format ingests fully or is
    /// gone ([`ConditionBus::vault_ingest_recovered`]).
    VaultIngestFailed(IngestRefusals),
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
    /// An edit would have left a tagged block in a shape its validator refuses,
    /// so the operation dispatcher refused it and the store never took it.
    /// `subject` is the tagged block; `rule` is the shape's name for the rule
    /// the edit would break.
    ///
    /// All-clear: none, like every refusal notice. The refused edit is gone;
    /// the notice stands for the session.
    EditRefusedByShape { tag: String, rule: String },
    /// A typed edit never reached the store because the editor could not take
    /// the document's write lock. `subject` is the block being typed into, and
    /// `detail` is the lock's own account of why. The keystroke is lost: the
    /// editor holds text the store does not.
    ///
    /// All-clear: none. Nothing re-applies the lost keystroke, so the notice
    /// stands until the user retypes and the session restarts.
    LocalEditNotApplied { detail: String },
    /// `*::rebuild_views` is dropping and recreating every watch view. A
    /// subscriber's list may lag or jump while it runs. `subject` is
    /// [`WATCH_VIEWS_SUBJECT`].
    ///
    /// All-clear: the op returns, whatever its outcome.
    WatchViewsRebuilding,
    /// Edits reach Loro + SQL but stop reaching disk. Two emitters: the org
    /// write-back stream died and its supervisor gave up (`subject` is the
    /// sentinel `"org-writeback"`), or one document's write-back fold stalled
    /// (`subject` is its file). The string says which and why.
    ///
    /// All-clear: for a stalled file, its next write; for the stream, a
    /// successful respawn, which nothing emits yet.
    WritebackDegraded(String),
    /// Write-back refused one file, or wrote it without some stored values.
    /// The store keeps every value. `detail` names each block and what the
    /// file lacks; `subject` is the file.
    ///
    /// All-clear: the next render of that file that holds every value.
    WritebackLossy { detail: String },
    /// An MCP integration provider did not come up at boot — its sidecar
    /// command is missing/dead, or its `${VAR}` credentials are unresolved. The
    /// integration's `cc_*` cache tables are never created, so every page that
    /// queries them renders blank; disclosed so that blankness is attributable
    /// instead of looking like a healthy empty result. `subject` carries
    /// the integration name (this is not tied to a shared doc).
    ///
    /// All-clear: the provider connecting.
    IntegrationConnectFailed { integration: String, error: String },
    /// An MCP integration's connect has been running longer than the slow
    /// threshold. It is not cancelled: a first-run sidecar can legitimately
    /// take that long. `subject` carries the integration name.
    ///
    /// All-clear: the connect finishing, whatever its outcome.
    IntegrationConnectSlow {
        integration: String,
        elapsed_secs: u64,
    },
    /// An MCP integration's `${VAR}` resolution is blocked on the OS keychain,
    /// which is usually waiting for the user to answer an access prompt.
    /// `subject` carries the integration name.
    ///
    /// All-clear: the keychain read returning.
    IntegrationWaitingOnKeychain { integration: String },
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
    /// The database was deleted and rebuilt at boot, because this binary
    /// could not use it (Martin: no migration). The replicas rebuild the
    /// blocks and views; `lost` names each table only the database held, and
    /// `caches` each integration cache that re-syncs from its source, with the
    /// rows each held.
    ///
    /// All-clear: none — it is a one-shot notice about this boot.
    DatabaseRebuiltAtBoot {
        reason: String,
        lost: Vec<LostTableRows>,
        caches: Vec<ClearedCacheRows>,
    },
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
    /// This device left a page someone shared with it: deleting the page here
    /// removed only this device's placement of it and its copy of the share.
    /// The owner's page, and every other recipient's, is unchanged. `subject`
    /// is the page; `title` its title when this device left.
    ///
    /// All-clear: none — a one-shot notice about what the delete did.
    LeftSharedPage { title: String },
    /// This device deleted a page (or block) it had shared, and the delete
    /// revoked the share: every recipient loses it. `subject` is what the
    /// share was known by; `title` its title when it was deleted.
    ///
    /// All-clear: none — a one-shot notice about what the delete did.
    DeletedSharedPage { title: String },
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
    /// A shared doc loaded from disk or a peer holds the mount of another
    /// share, which v1 forbids (ADR 0028 A7): it was saved before
    /// `share_subtree` refused nesting, or came from an older peer. Holon does
    /// not repair it. `subject` is the outer shared tree; `mount` is the inner
    /// mount's block id.
    ///
    /// All-clear: none. The state lives in the stored doc until the user
    /// unshares it, so the condition holds for the session.
    NestedShareLoaded { mount: String },
    /// The global doc holds more than one live mount of one shared tree: two
    /// devices accepted the same ticket before they synced. Holon uses the
    /// mount with the smallest tree id on every device and does not delete
    /// the others. `subject` is the shared tree; a change in the set of
    /// mounts raises it again.
    ///
    /// All-clear: the next mount lookup that finds one live mount.
    DuplicateMount {
        canonical: String,
        duplicates: Vec<String>,
    },
    /// An entity profile source block failed to load and is not applied.
    /// `subject` is the block; `error` names the profile, the variant or
    /// computed field, the expression and the column.
    ///
    /// All-clear: the next successful load of the same block. A deleted block's
    /// condition stands until restart.
    ProfileRefused { error: String },
    /// One block id is on disk in two or more org files. `subject` is the
    /// block id; `owner_file` belongs to the page that owns the block and stays
    /// authoritative, `copy_files` hold the other copies. Holon deletes none:
    /// a copy is half of a move whose source is not saved yet, or a stale copy
    /// only the user can judge.
    ///
    /// All-clear: [`BlockInOneFile`](crate::condition_profile::AllClear).
    BlockInTwoFiles {
        owner_file: String,
        copy_files: Vec<String>,
    },
    /// [`BlockInTwoFiles`](Self::BlockInTwoFiles) where the owner's file let
    /// the block go, but a copy and Holon's version were both edited apart, so
    /// no copy was adopted. The user's next deletion of one of them decides.
    ///
    /// All-clear: [`BlockInOneFile`](crate::condition_profile::AllClear).
    BlockEditedInTwoFiles {
        owner_file: String,
        copy_files: Vec<String>,
    },
    /// `subject`, a block id, was deleted in Holon while `file` held a copy of
    /// it; `file` brought it back as its own block.
    ///
    /// All-clear: the block leaves `file`, or is deleted or moved in Holon.
    DeletedBlockKeptInFile { file: String },
    /// `subject`, a block id, was deleted from `file`, the file that owns it,
    /// and Holon wrote it back: each of `copy_files` holds a copy of the
    /// heading it is under, and a block another file holds is never deleted.
    ///
    /// All-clear: [`BlockInOneFile`](crate::condition_profile::AllClear) of
    /// that heading.
    DeletionUndoneBlockInOtherFile {
        file: String,
        copy_files: Vec<String>,
    },
    /// `subject`, a block id, was deleted from `file` and put back by Holon
    /// (see [`DeletionUndoneBlockInOtherFile`](Self::DeletionUndoneBlockInOtherFile)),
    /// and it was edited since: the deletion no longer stands, and the block
    /// stays.
    ///
    /// All-clear: none in this process.
    DeletionEndedByEdit { file: String },
    /// `subject`, a block id, was edited in `file` after Holon `change`d it.
    /// Holon's change stands; the block as the file holds it, `file_text`
    /// (text, properties, tags, state), is not ingested, and the next
    /// write-back replaces it.
    ///
    /// All-clear: none in this process.
    FileEditOverruled {
        file: String,
        file_text: String,
        change: HolonChange,
    },
    /// `subject`, a vault root, is not synced: the file sync failed before it
    /// watched the vault. `cause` names the failure and what no longer syncs.
    ///
    /// All-clear: none in this process; a restart starts the controller again.
    VaultSyncNotStarted { cause: String },
    /// `subject`, a record Holon keeps in the vault, could not be read. It was
    /// kept as it was at `kept_as`, and Holon started without what it could
    /// not read.
    ///
    /// All-clear: none in this process.
    VaultStateUnreadable { kept_as: String, reason: String },
    /// `subject`, a vault root, is synced, but `step` of its start failed
    /// (`cause`), so what that step does was not done this run.
    ///
    /// All-clear: none in this process; a restart runs the step again.
    VaultStartIncomplete { step: String, cause: String },
    /// `subject`, a vault root, is synced, but `done` of its `total` read-only
    /// files (recipes) are ingested so far; the rest appear as they are read.
    ///
    /// All-clear: the backlog ends, whatever its outcome.
    VaultBacklogIngesting { done: usize, total: usize },
    /// `subject`, a vault root, holds `files` Holon wrote without recording
    /// which bytes it wrote (`cause`), so the next start reads them again.
    ///
    /// All-clear:
    /// [`WrittenHashRecord`](crate::condition_profile::ClearingEvent).
    WrittenFilesUnrecorded { files: Vec<String>, cause: String },
    /// `subject`, a Loro update log, ended in a record a crash cut short
    /// (`reason`); its last `bytes` were dropped at load. That record was
    /// never reported saved, so neither SQL nor the org files reflect it.
    ///
    /// All-clear: none in this process.
    LoroUpdateLogTailDropped { bytes: u64, reason: String },
    /// The view engine stopped at its first error (`reason`, which names the
    /// cause). Its views keep the last version before the error.
    ///
    /// All-clear: none in this process; the engine is rebuilt at start.
    ViewEngineStopped(String),
    /// Rendered rows stopped following the operation catalog (`reason`), so an
    /// operation registered from now on is dispatchable but offered on no row.
    ///
    /// All-clear: none in this process.
    OperationCatalogStopped(String),
    /// The SQL database has run one `command` (its kind, such as
    /// `Transaction`) for `running_secs` without finishing, so every read and
    /// write waits behind it. `report` is the log's account of it: the
    /// redacted SQL, the queue behind it, the commands before it.
    ///
    /// All-clear: the command finishes.
    DatabaseStuck {
        command: String,
        running_secs: u64,
        report: String,
    },
    /// The watch over one SQL actor failed (`cause`, which names what no
    /// longer works): a stuck command on it is no longer disclosed, or every
    /// command on it fails.
    ///
    /// All-clear: none in this process.
    DatabaseWatchFailed { cause: String },
    /// Derived field `field` of block `block_id` (the two make `subject`) has
    /// no value: its computation failed or gave what JSON cannot hold
    /// (`reason`). The block holds no stored value for it, and a row that
    /// shows it carries `Null`.
    ///
    /// All-clear: [`DerivedFieldEval`](crate::condition_profile::ClearingEvent)
    /// of the same field and block.
    DerivedFieldNotComputed {
        block_id: String,
        field: String,
        reason: String,
    },
    /// The previous run of Holon panicked at `subject` (a `file:line:column`)
    /// on `thread` with `message`, as its panic record says, after the
    /// `earlier` runs that no frontend has shown either. One condition stands
    /// for all of them, so a crash loop fits where the user looks. Raised
    /// once, at start.
    ///
    /// All-clear: none in this process.
    PreviousRunPanicked {
        message: String,
        thread: String,
        earlier: EarlierPanics,
    },
    /// A thread or task of this process panicked at `subject` (a
    /// `file:line:column`) with `message`; what it was doing stopped.
    ///
    /// All-clear: none in this process.
    TaskPanicked { message: String, thread: String },
    /// Holon cannot keep a panic record in the directory `subject` names
    /// (`reason`), so a crash is not disclosed at the next start.
    ///
    /// All-clear: none in this process.
    PanicRecordUnwritable { reason: String },
    /// The entry `subject` among the panic records cannot be read
    /// (`reason`); the records beside it are still shown.
    ///
    /// All-clear: none in this process.
    PanicRecordUnreadable { reason: String },
    /// A panic of this process cannot reach this bus (`reason`); its record
    /// still discloses it at the next start. Subject:
    /// [`PANIC_CONDITIONS_SUBJECT`].
    ///
    /// All-clear: none in this process.
    PanicConditionsUnavailable { reason: String },
    /// The stored table of type `subject` differs from the type's declaration
    /// in a way adding columns does not fix (`diff`), so this session does not
    /// serve the type: its writes fail naming this condition and its `rows`
    /// stay untouched.
    ///
    /// All-clear: [`RemedyApplied`](crate::condition_profile::AllClear) — the
    /// user drops the table and its rows.
    TypeTableRefused {
        table: String,
        diff: String,
        rows: u64,
    },
    /// The stored table of type `subject` gained the `added` columns its
    /// declaration names; its `rows` read them as NULL or their default.
    /// `undeclared` stored columns are kept as they are.
    ///
    /// All-clear: none in this process.
    TypeTableColumnsAdded {
        table: String,
        added: Vec<String>,
        undeclared: Vec<String>,
        rows: u64,
    },
    /// Schema module `subject` could not set up its tables or views
    /// (`error`); whatever reads or writes them fails.
    ///
    /// All-clear: none in this process.
    SchemaModuleFailed { error: String },
}

/// Subject of the device-wide conditions on this bus, which have no share to
/// name. Used by [`ConditionKind::OwnerRecoveryCodeNotShown`].
pub const OWNER_IDENTITY_SUBJECT: &str = "owner-identity";

pub const WATCH_VIEWS_SUBJECT: &str = "watch-views";

/// Subject of [`ConditionKind::PanicConditionsUnavailable`].
pub const PANIC_CONDITIONS_SUBJECT: &str = "panic-conditions";

/// Subject of [`ConditionKind::ViewEngineStopped`].
pub const VIEW_ENGINE_SUBJECT: &str = "view-engine";

/// Subject of [`ConditionKind::OperationCatalogStopped`].
pub const OPERATION_CATALOG_SUBJECT: &str = "operation-catalog";

/// Subject prefix of [`ConditionKind::DatabaseStuck`] and
/// [`ConditionKind::DatabaseWatchFailed`]; the subject names one SQL actor.
pub const DATABASE_SUBJECT: &str = "database";

impl ConditionKind {
    /// Kind constants, so an all-clear site names the condition it lifts
    /// through the compiler instead of retyping the string.
    pub const FOREIGN_ID_COLLISION: &'static str = "foreign-id-collision";
    pub const INTEGRATION_CONNECT_FAILED: &'static str = "integration-connect-failed";
    pub const INTEGRATION_CONNECT_SLOW: &'static str = "integration-connect-slow";
    pub const INTEGRATION_WAITING_ON_KEYCHAIN: &'static str = "integration-waiting-on-keychain";
    pub const INTEGRATION_NEEDS_AUTH: &'static str = "integration-needs-auth";
    pub const INTEGRATION_NOT_ENABLED: &'static str = "integration-not-enabled";
    pub const INTEGRATION_SIDECAR_NOT_BUNDLED: &'static str = "integration-sidecar-not-bundled";
    pub const INTEGRATION_SIDECAR_UNUSABLE: &'static str = "integration-sidecar-unusable";
    pub const INTEGRATION_SIDECAR_SUPERSEDED: &'static str = "integration-sidecar-superseded";
    pub const PAIRING_REIMPORTED_LOCAL_CONTENT: &'static str = "pairing-reimported-local-content";
    pub const LEFT_SHARED_PAGE: &'static str = "left-shared-page";
    pub const DELETED_SHARED_PAGE: &'static str = "deleted-shared-page";
    pub const PAIRING_REIMPORT_DEFERRED: &'static str = "pairing-reimport-deferred";
    pub const REHYDRATION_FAILED: &'static str = "rehydration-failed";
    pub const SECRETS_HELD_IN_MEMORY: &'static str = "secrets-held-in-memory";
    pub const UNDO_HISTORY_CLEARED_AT_BOOT: &'static str = "undo-history-cleared-at-boot";
    pub const DATABASE_REBUILT_AT_BOOT: &'static str = "database-rebuilt-at-boot";
    pub const SHARED_SUBTREE_NOT_MATERIALIZED: &'static str = "shared-subtree-not-materialized";
    pub const SNAPSHOT_LOAD_FAILED: &'static str = "snapshot-load-failed";
    pub const SNAPSHOT_SAVE_FAILED: &'static str = "snapshot-save-failed";
    pub const SQL_PROJECTION_FAILED: &'static str = "sql-projection-failed";
    pub const VAULT_INGEST_FAILED: &'static str = "vault-ingest-failed";
    pub const VAULT_FILE_EMPTIED: &'static str = "vault-file-emptied";
    pub const WRITEBACK_DEGRADED: &'static str = "writeback-degraded";
    pub const WRITEBACK_LOSSY: &'static str = "writeback-lossy";
    pub const EDIT_REFUSED_READ_ONLY_FORMAT: &'static str = "edit-refused-read-only-format";
    pub const EDIT_REFUSED_BY_SHAPE: &'static str = "edit-refused-by-shape";
    pub const LOCAL_EDIT_NOT_APPLIED: &'static str = "local-edit-not-applied";
    pub const BEARER_TICKET_ENROLLMENT: &'static str = "bearer-ticket-enrollment";
    pub const OWNER_RECOVERY_CODE_NOT_SHOWN: &'static str = "owner-recovery-code-not-shown";
    pub const NESTED_SHARE_LOADED: &'static str = "nested-share-loaded";
    pub const DUPLICATE_MOUNT: &'static str = "duplicate-mount";
    pub const WATCH_VIEWS_REBUILDING: &'static str = "watch-views-rebuilding";
    pub const VAULT_BACKLOG_INGESTING: &'static str = "vault-backlog-ingesting";
    pub const PROFILE_REFUSED: &'static str = "profile-refused";
    pub const BLOCK_IN_TWO_FILES: &'static str = "block-in-two-files";
    pub const BLOCK_EDITED_IN_TWO_FILES: &'static str = "block-edited-in-two-files";
    pub const DELETED_BLOCK_KEPT_IN_FILE: &'static str = "deleted-block-kept-in-file";
    pub const DELETION_UNDONE_BLOCK_IN_OTHER_FILE: &'static str =
        "deletion-undone-block-in-other-file";
    pub const DELETION_ENDED_BY_EDIT: &'static str = "deletion-ended-by-edit";
    pub const FILE_EDIT_OVERRULED: &'static str = "file-edit-overruled";
    pub const VAULT_SYNC_NOT_STARTED: &'static str = "vault-sync-not-started";
    pub const VAULT_STATE_UNREADABLE: &'static str = "vault-state-unreadable";
    pub const VAULT_START_INCOMPLETE: &'static str = "vault-start-incomplete";
    pub const WRITTEN_FILES_UNRECORDED: &'static str = "written-files-unrecorded";
    pub const LORO_UPDATE_LOG_TAIL_DROPPED: &'static str = "loro-update-log-tail-dropped";
    pub const VIEW_ENGINE_STOPPED: &'static str = "view-engine-stopped";
    pub const OPERATION_CATALOG_STOPPED: &'static str = "operation-catalog-stopped";
    pub const DATABASE_STUCK: &'static str = "database-stuck";
    pub const DATABASE_WATCH_FAILED: &'static str = "database-watch-failed";
    pub const DERIVED_FIELD_NOT_COMPUTED: &'static str = "derived-field-not-computed";
    pub const PREVIOUS_RUN_PANICKED: &'static str = "previous-run-panicked";
    pub const TASK_PANICKED: &'static str = "task-panicked";
    pub const PANIC_RECORD_UNWRITABLE: &'static str = "panic-record-unwritable";
    pub const PANIC_RECORD_UNREADABLE: &'static str = "panic-record-unreadable";
    pub const PANIC_CONDITIONS_UNAVAILABLE: &'static str = "panic-conditions-unavailable";
    pub const TYPE_TABLE_REFUSED: &'static str = "type-table-refused";
    pub const TYPE_TABLE_COLUMNS_ADDED: &'static str = "type-table-columns-added";
    pub const SCHEMA_MODULE_FAILED: &'static str = "schema-module-failed";

    /// The condition's stable identity, paired with the subject to form a
    /// [`ConditionKey`]. Total: every degradation is a sticky
    /// condition, so a new variant cannot opt out of replay by accident — it
    /// can only fail to compile until it names its kind (and, per this enum's
    /// doc contract, its all-clear).
    pub fn condition_kind(&self) -> &'static str {
        match self {
            Self::IntegrationConnectFailed { .. } => Self::INTEGRATION_CONNECT_FAILED,
            Self::IntegrationConnectSlow { .. } => Self::INTEGRATION_CONNECT_SLOW,
            Self::IntegrationWaitingOnKeychain { .. } => Self::INTEGRATION_WAITING_ON_KEYCHAIN,
            Self::IntegrationNeedsAuth { .. } => Self::INTEGRATION_NEEDS_AUTH,
            Self::IntegrationSidecarSuperseded { .. } => Self::INTEGRATION_SIDECAR_SUPERSEDED,
            Self::IntegrationNotEnabled { .. } => Self::INTEGRATION_NOT_ENABLED,
            Self::IntegrationSidecarNotBundled { .. } => Self::INTEGRATION_SIDECAR_NOT_BUNDLED,
            Self::IntegrationSidecarUnusable { .. } => Self::INTEGRATION_SIDECAR_UNUSABLE,
            Self::SecretsHeldInMemory { .. } => Self::SECRETS_HELD_IN_MEMORY,
            Self::UndoHistoryClearedAtBoot { .. } => Self::UNDO_HISTORY_CLEARED_AT_BOOT,
            Self::DatabaseRebuiltAtBoot { .. } => Self::DATABASE_REBUILT_AT_BOOT,
            Self::SnapshotSaveFailed(_) => Self::SNAPSHOT_SAVE_FAILED,
            Self::SnapshotLoadFailed(_) => Self::SNAPSHOT_LOAD_FAILED,
            Self::RehydrationFailed(_) => Self::REHYDRATION_FAILED,
            Self::SqlProjectionFailed(_) => Self::SQL_PROJECTION_FAILED,
            Self::ForeignIdCollision(_) => Self::FOREIGN_ID_COLLISION,
            Self::VaultIngestFailed(_) => Self::VAULT_INGEST_FAILED,
            Self::VaultFileEmptied => Self::VAULT_FILE_EMPTIED,
            Self::SharedSubtreeNotMaterialized { .. } => Self::SHARED_SUBTREE_NOT_MATERIALIZED,
            Self::WritebackDegraded(_) => Self::WRITEBACK_DEGRADED,
            Self::WritebackLossy { .. } => Self::WRITEBACK_LOSSY,
            Self::PairingReimportedLocalContent { .. } => Self::PAIRING_REIMPORTED_LOCAL_CONTENT,
            Self::LeftSharedPage { .. } => Self::LEFT_SHARED_PAGE,
            Self::DeletedSharedPage { .. } => Self::DELETED_SHARED_PAGE,
            Self::PairingReimportDeferred { .. } => Self::PAIRING_REIMPORT_DEFERRED,
            Self::EditRefusedReadOnlyFormat { .. } => Self::EDIT_REFUSED_READ_ONLY_FORMAT,
            Self::EditRefusedByShape { .. } => Self::EDIT_REFUSED_BY_SHAPE,
            Self::LocalEditNotApplied { .. } => Self::LOCAL_EDIT_NOT_APPLIED,
            Self::BearerTicketEnrollment { .. } => Self::BEARER_TICKET_ENROLLMENT,
            Self::OwnerRecoveryCodeNotShown => Self::OWNER_RECOVERY_CODE_NOT_SHOWN,
            Self::NestedShareLoaded { .. } => Self::NESTED_SHARE_LOADED,
            Self::DuplicateMount { .. } => Self::DUPLICATE_MOUNT,
            Self::WatchViewsRebuilding => Self::WATCH_VIEWS_REBUILDING,
            Self::VaultBacklogIngesting { .. } => Self::VAULT_BACKLOG_INGESTING,
            Self::ProfileRefused { .. } => Self::PROFILE_REFUSED,
            Self::BlockInTwoFiles { .. } => Self::BLOCK_IN_TWO_FILES,
            Self::BlockEditedInTwoFiles { .. } => Self::BLOCK_EDITED_IN_TWO_FILES,
            Self::DeletedBlockKeptInFile { .. } => Self::DELETED_BLOCK_KEPT_IN_FILE,
            Self::DeletionUndoneBlockInOtherFile { .. } => {
                Self::DELETION_UNDONE_BLOCK_IN_OTHER_FILE
            }
            Self::DeletionEndedByEdit { .. } => Self::DELETION_ENDED_BY_EDIT,
            Self::FileEditOverruled { .. } => Self::FILE_EDIT_OVERRULED,
            Self::VaultSyncNotStarted { .. } => Self::VAULT_SYNC_NOT_STARTED,
            Self::VaultStateUnreadable { .. } => Self::VAULT_STATE_UNREADABLE,
            Self::VaultStartIncomplete { .. } => Self::VAULT_START_INCOMPLETE,
            Self::WrittenFilesUnrecorded { .. } => Self::WRITTEN_FILES_UNRECORDED,
            Self::LoroUpdateLogTailDropped { .. } => Self::LORO_UPDATE_LOG_TAIL_DROPPED,
            Self::ViewEngineStopped(_) => Self::VIEW_ENGINE_STOPPED,
            Self::OperationCatalogStopped(_) => Self::OPERATION_CATALOG_STOPPED,
            Self::DatabaseStuck { .. } => Self::DATABASE_STUCK,
            Self::DatabaseWatchFailed { .. } => Self::DATABASE_WATCH_FAILED,
            Self::DerivedFieldNotComputed { .. } => Self::DERIVED_FIELD_NOT_COMPUTED,
            Self::PreviousRunPanicked { .. } => Self::PREVIOUS_RUN_PANICKED,
            Self::TaskPanicked { .. } => Self::TASK_PANICKED,
            Self::PanicRecordUnwritable { .. } => Self::PANIC_RECORD_UNWRITABLE,
            Self::PanicRecordUnreadable { .. } => Self::PANIC_RECORD_UNREADABLE,
            Self::PanicConditionsUnavailable { .. } => Self::PANIC_CONDITIONS_UNAVAILABLE,
            Self::TypeTableRefused { .. } => Self::TYPE_TABLE_REFUSED,
            Self::TypeTableColumnsAdded { .. } => Self::TYPE_TABLE_COLUMNS_ADDED,
            Self::SchemaModuleFailed { .. } => Self::SCHEMA_MODULE_FAILED,
        }
    }
}

/// One vault file a format adapter refused, and the adapter's error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefusedFile {
    pub path: String,
    pub reason: String,
}

/// The refused files of one format, as its
/// [`ConditionKind::VaultIngestFailed`] shows them. Only the bus builds one,
/// from its per-file record, so `count` always matches that record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngestRefusals {
    format: String,
    count: NonZeroUsize,
    examples: Vec<RefusedFile>,
}

impl IngestRefusals {
    /// How many files a condition shows by name.
    pub const EXAMPLES: usize = 3;

    pub fn format(&self) -> &str {
        &self.format
    }

    pub fn count(&self) -> NonZeroUsize {
        self.count
    }

    /// The first [`EXAMPLES`](Self::EXAMPLES) refused files, by path.
    pub fn examples(&self) -> &[RefusedFile] {
        &self.examples
    }
}

/// What Holon did to a block that an org file still holds as it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HolonChange {
    Deleted,
    Moved,
}

impl std::fmt::Display for HolonChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Deleted => "deleted",
            Self::Moved => "moved",
        })
    }
}

/// The runs before the last one that panicked too, and whose records no
/// frontend has shown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EarlierPanics {
    /// Newest first.
    pub panics: Vec<EarlierPanic>,
    pub dropped: Option<DroppedPanics>,
}

/// Where one earlier run panicked, and with what message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EarlierPanic {
    /// `file:line:column`.
    pub location: String,
    pub message: String,
}

impl EarlierPanics {
    pub fn runs(&self) -> usize {
        self.panics.len() + self.dropped.as_ref().map_or(0, DroppedPanics::count)
    }
}

/// Panicked runs whose records were dropped to bound how many are kept: how
/// many, and when the first and the last of those whose end time could be
/// read ended.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DroppedPanics {
    count: usize,
    first_ended: Option<chrono::DateTime<chrono::Utc>>,
    last_ended: Option<chrono::DateTime<chrono::Utc>>,
    /// Of `count`, the runs whose end time could not be read.
    #[serde(default)]
    undated: usize,
}

impl DroppedPanics {
    /// One run, which ended at `ended` when that could be read.
    pub fn one(ended: Option<chrono::DateTime<chrono::Utc>>) -> Self {
        Self {
            count: 1,
            first_ended: ended,
            last_ended: ended,
            undated: usize::from(ended.is_none()),
        }
    }

    pub fn and(self, other: Self) -> Self {
        let earliest = |a: Option<_>, b: Option<_>| match (a, b) {
            (Some(a), Some(b)) => Some(std::cmp::min(a, b)),
            (a, b) => a.or(b),
        };
        let latest = |a: Option<_>, b: Option<_>| match (a, b) {
            (Some(a), Some(b)) => Some(std::cmp::max(a, b)),
            (a, b) => a.or(b),
        };
        Self {
            count: self.count + other.count,
            first_ended: earliest(self.first_ended, other.first_ended),
            last_ended: latest(self.last_ended, other.last_ended),
            undated: self.undated + other.undated,
        }
    }

    pub fn count(&self) -> usize {
        self.count
    }

    /// When the first and the last of the dated runs ended.
    pub fn ended_between(
        &self,
    ) -> Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)> {
        self.first_ended.zip(self.last_ended)
    }

    pub fn undated(&self) -> usize {
        self.undated
    }
}

/// A table a database rebuild deleted and no replica restores, with the rows
/// it held.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LostTableRows {
    pub table: String,
    pub what: String,
    pub rows: u64,
}

/// An integration cache a database rebuild cleared, with the rows it held.
/// Its provider re-syncs it; a row the source no longer holds is gone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClearedCacheRows {
    pub table: String,
    pub provider: String,
    pub rows: u64,
}

/// Which component computes a derived field. Each seat raises and clears only
/// its own [`ConditionKind::DerivedFieldNotComputed`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DerivedFieldSeat {
    /// The derived-field sidecar reconciler.
    Sidecar,
    /// The profile resolver's computed fields.
    Resolver,
}

impl DerivedFieldSeat {
    fn name(self) -> &'static str {
        match self {
            Self::Sidecar => "sidecar",
            Self::Resolver => "resolver",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Condition {
    pub subject: String,
    pub reason: ConditionKind,
}

impl Condition {
    /// Derived field `field` of block `block_id` has no value because of
    /// `reason`.
    pub fn derived_field_not_computed(
        seat: DerivedFieldSeat,
        block_id: &str,
        field: &str,
        reason: String,
    ) -> Self {
        Self {
            subject: ConditionKey::derived_field_not_computed(seat, block_id, field).subject,
            reason: ConditionKind::DerivedFieldNotComputed {
                block_id: block_id.to_string(),
                field: field.to_string(),
                reason,
            },
        }
    }

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
    /// The key of [`ConditionKind::DerivedFieldNotComputed`] for `field` of
    /// `block_id`.
    pub fn derived_field_not_computed(seat: DerivedFieldSeat, block_id: &str, field: &str) -> Self {
        Self {
            subject: format!("{block_id}/{field}@{}", seat.name()),
            kind: ConditionKind::DERIVED_FIELD_NOT_COMPUTED,
        }
    }

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
    conditions: crate::live_data::AuthoredLiveData<Condition>,
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
    /// Format -> refused path -> reason. The `VaultIngestFailed` condition of
    /// each format is derived from its entry, under this lock.
    refused_files: std::sync::Mutex<BTreeMap<String, BTreeMap<String, String>>>,
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
            refused_files: std::sync::Mutex::new(BTreeMap::new()),
        }
    }

    /// Record that `format`'s adapter refused `file`, and raise or update that
    /// format's [`ConditionKind::VaultIngestFailed`]. A file refused again
    /// replaces its reason.
    pub fn vault_ingest_refused(&self, format: &str, file: RefusedFile) {
        let mut refused = self.refused_files.lock().unwrap();
        let other_format = refused
            .iter()
            .find(|(f, files)| f.as_str() != format && files.contains_key(&file.path));
        assert!(
            other_format.is_none(),
            "{} is refused as {format} while it stands refused as {:?}: a file has one format",
            file.path,
            other_format.map(|(f, _)| f)
        );
        let files = refused.entry(format.to_string()).or_default();
        files.insert(file.path, file.reason);
        self.emit(Self::ingest_refusal_condition(format, files));
    }

    /// `file` is no longer refused: it ingested fully, or it is gone. Takes it
    /// out of its format's group, and clears the condition with the last one.
    /// A file that was never refused changes nothing.
    pub fn vault_ingest_recovered(&self, file: &str) {
        let mut refused = self.refused_files.lock().unwrap();
        let Some(format) = refused
            .iter_mut()
            .find_map(|(format, files)| files.remove(file).map(|_| format.clone()))
        else {
            return;
        };
        let files = &refused[&format];
        if files.is_empty() {
            refused.remove(&format);
            self.clear(&ConditionKey {
                subject: format,
                kind: ConditionKind::VAULT_INGEST_FAILED,
            });
        } else {
            self.emit(Self::ingest_refusal_condition(&format, files));
        }
    }

    /// Every file `format` refuses, by path, with its reason.
    pub fn refused_files(&self, format: &str) -> Vec<RefusedFile> {
        self.refused_files
            .lock()
            .unwrap()
            .get(format)
            .into_iter()
            .flatten()
            .map(|(path, reason)| RefusedFile {
                path: path.clone(),
                reason: reason.clone(),
            })
            .collect()
    }

    fn ingest_refusal_condition(format: &str, files: &BTreeMap<String, String>) -> Condition {
        Condition {
            subject: format.to_string(),
            reason: ConditionKind::VaultIngestFailed(IngestRefusals {
                format: format.to_string(),
                count: NonZeroUsize::new(files.len())
                    .expect("a format's group is removed with its last file"),
                examples: files
                    .iter()
                    .take(IngestRefusals::EXAMPLES)
                    .map(|(path, reason)| RefusedFile {
                        path: path.clone(),
                        reason: reason.clone(),
                    })
                    .collect(),
            }),
        }
    }

    /// The conditions in effect, as the live holder. This is what the
    /// `conditions` row source is registered over, so a rendered collection
    /// and a bus subscriber read the same set by construction.
    pub fn conditions(&self) -> Arc<LiveData<Condition>> {
        self.conditions.shared()
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
            ConditionKind::VaultFileEmptied,
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
