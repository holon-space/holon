//! Whole-store device pairing: a second device this user owns adopts the
//! owner's entire replication set.
//!
//! A pair is not a share. A share hands a THIRD party one subtree and mounts
//! it; a pair hands this user's own second device every registered container
//! and mounts nothing. The two reuse the same iroh wire and nothing else, so
//! they are separate operations — see [`crate::loro_share_backend`] for the
//! share side.
//!
//! Whole-store means iterating [`ContainerRegistry::replication_set`]. There is
//! no filter and no per-document predicate: a document that must stay
//! device-local (the UI layout) is excluded by never being registered, the same
//! by-construction exclusion the registry already gives `NonReplicated<T>`.
//!
//! ## Pairing a device that was already used on its own
//! A pair is an ADOPTION, not a merge. Two devices that each booted alone
//! minted their OWN node for every fixed id the app seeds (`block:journals`,
//! today's day block, the layout), so importing the owner's history over a
//! store that still holds this device's mintings leaves two live nodes under
//! each of those ids. So the receiver archives its documents, wipes its tree,
//! adopts the owner's history, and re-imports its own content as ordinary
//! creates through the shared [`BlockOrdering`] — keeping every uuid, and
//! hanging what it wrote under the OWNER's node wherever the two devices share
//! an id.
//!
//! ## Why there is no read-only pair
//! The iroh leg authorizes in [`crate::share_enrollment::acceptor_enroll`],
//! which proves possession of a `CapabilitySecret` and has no read/write
//! dimension; the read/write rule lives in `holon_sharing::acceptor::admit`,
//! which has no caller on this path. A read grant is therefore unenforceable
//! here, and [`PairCapability::Read`] is refused at the offer (D86) rather than
//! minted as an invite that silently grants write.

use anyhow::Context;
use anyhow::bail;
use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use holon_api::EntityName;
use holon_api::OperationDescriptor;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_core::MaybeSendSync;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result;
use holon_core::block_ordering::BlockCreateRequest;
use holon_core::block_ordering::BlockOrdering;
use serde::Deserialize;
use serde::Serialize;

use crate::container_registry::ContainerRegistry;
use crate::degraded_signal_bus::DegradedSignalBus;
use crate::degraded_signal_bus::ShareDegraded;
use crate::degraded_signal_bus::ShareDegradedReason;
use crate::iroh_advertiser::ALPN_PREFIX;
use crate::iroh_advertiser::IrohAdvertiser;
use crate::iroh_advertiser::SharedRoster;
use crate::iroh_sync_adapter::create_endpoint;
use crate::iroh_sync_adapter::make_alpn;
use crate::iroh_sync_adapter::sync_doc_initiate_enrolled;
use crate::loro_document_store::DocScope;
use crate::loro_document_store::LoroDocumentStore;
use crate::share_enrollment::CapabilitySecret;
use crate::share_enrollment::ExpiryTime;
use crate::share_enrollment::ShareRoster;
use crate::ticket::Ticket;

/// The entity the pairing operations dispatch under.
pub const DEVICE_ENTITY: &str = "device";

/// How long a pairing invite admits new devices, in seconds.
const INVITE_TTL_SECS: i64 = 15 * 60;
/// Peers one invite may enroll. An invite is a bearer credential over the whole
/// store: one invite pairs the one device it was minted for, and a second
/// attempt is refused rather than silently adopting another device.
const INVITE_MAX_PEERS: usize = 1;
/// Tickets one invite may carry. A store's replication set is a handful of
/// containers; anything near this is a crafted invite, and every ticket becomes
/// an ALPN on the receiver's endpoint before a single one is validated.
const MAX_INVITE_CONTAINERS: usize = 64;
/// A dial that has not completed by here is a failure, not a slow network.
const DIAL_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

/// The page the bundled layout hangs under, seeded on every device at boot.
/// D77.b (a device-local layout document) takes the layout out of the
/// replicated store and retires this family.
const LAYOUT_ROOT: &str = holon_api::DEFAULT_DOC_BLOCK_ID;

/// The layout asset `holon-app`'s seed parses into the store.
const BUNDLED_LAYOUT: &str = include_str!("../../../assets/default/index.org");

/// Every id the layout seed writes: `:ID:` on a heading, `:id` on a source
/// block. The asset authors all of them, so a block under [`LAYOUT_ROOT`] that
/// is not here is the user's.
fn bundled_layout_ids() -> impl Iterator<Item = String> {
    BUNDLED_LAYOUT.lines().filter_map(|line| {
        let line = line.trim();
        let bare = if let Some(rest) = line.strip_prefix(":ID:") {
            rest.trim()
        } else if line.starts_with("#+BEGIN_SRC") {
            line.split(":id ").nth(1)?.split_whitespace().next()?
        } else {
            return None;
        };
        Some(format!("block:{bare}"))
    })
}

/// The journals shell's fixed id. Seeded from the bundled `journals.org`, which
/// is where the id is authored — there is no constant to import.
const JOURNALS_BLOCK_ID: &str = "block:journals";

/// The journals shell and the query, render and rule blocks seeded alongside
/// it. A closed list, not a parent-closure: the auto-create rule mints day
/// blocks under `block:journals`, and what a user types into a day block hangs
/// under that.
const JOURNALS_MACHINERY: [&str; 5] = [
    JOURNALS_BLOCK_ID,
    "block:journals::src::0",
    "block:journals::render::0",
    "block:journals::auto-create",
    "block:journals::action::0",
];

/// What the paired device is invited to do. Recorded in the invite; see the
/// module docs for what currently enforces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairCapability {
    Read,
    Write,
}

impl PairCapability {
    /// Parse the operation parameter. Unknown values are refused rather than
    /// defaulted — a typo must not silently downgrade or upgrade a device.
    pub fn parse(raw: &str) -> anyhow::Result<Self> {
        match raw {
            "read" => Ok(Self::Read),
            "write" => Ok(Self::Write),
            other => bail!("capability must be `read` or `write`, got {other:?}"),
        }
    }
}

/// Why a pairing was refused. Every variant names what it found, so the user
/// can act on it without reading a log.
#[derive(Debug, thiserror::Error)]
pub enum PairingRefused {
    /// The receiver holds per-subtree mounts. Their shared documents are
    /// outside the owner's replication set, so pairing would adopt the owner's
    /// store and silently drop them.
    #[error(
        "this device holds {} mounted share(s) and cannot be paired; unshare them first: {}",
        mounts.len(),
        mounts.join(", ")
    )]
    ReceiverHoldsMounts { mounts: Vec<String> },

    /// A block this device wrote hangs under a parent that the owner's store
    /// does not hold and that this device did not write either — so the
    /// re-import has nowhere to put it. Refused rather than re-homed under an
    /// invented parent: guessing would move the user's content silently.
    #[error(
        "the re-import cannot place {} block(s) written on this device: their parent is neither \
         in the owner's store nor among the blocks being re-imported: {}",
        orphans.len(),
        preview(orphans)
    )]
    ReimportHasNoParent { orphans: Vec<String> },

    /// The pair left an id naming more than one live node — the CRDT-merge
    /// outcome the archive-and-re-import exists to prevent. Reported as a
    /// failure of the pair, because a store in that state serves two different
    /// blocks under one id to every reader downstream.
    #[error(
        "after pairing, {} block id(s) name more than one live node: {}",
        duplicates.len(),
        preview(duplicates)
    )]
    PairLeftDuplicateNodes { duplicates: Vec<String> },

    #[error(
        "read-only pairing is not available (D86): the pairing wire authorizes by capability \
         possession and cannot distinguish read from write, so a `read` invite would grant the \
         paired device full write access; pair with `write`"
    )]
    ReadOnlyPairingUnsupported,

    /// One offer at a time. A second one would advertise the same containers
    /// again, and the first offer's capabilities would then be live with no
    /// invite naming them and no way to withdraw them.
    #[error(
        "this device is already offering a pair over {containers} container(s); run \
         `device.pair_cancel` to withdraw that offer before making a new one"
    )]
    OfferAlreadyLive { containers: usize },

    #[error("this device is not offering a pair, so there is nothing to cancel")]
    NoLiveOffer,

    /// This device already belongs to an owner. Adopting a second store would
    /// capture the first owner's content as "this device's own" and re-import
    /// it into the second owner's store, which replicates it there.
    #[error(
        "this device is already paired to {owner} (since {paired_at}) and cannot be paired again; \
         its pre-pair documents are in {archive}"
    )]
    AlreadyPaired {
        owner: String,
        paired_at: String,
        archive: String,
    },

    /// The invite advertises a container this device holds as a document but
    /// cannot fetch into a staging store: only the root container is a file in
    /// the store directory. A receiver reaches this state only by holding a
    /// mount, which [`PairingRefused::ReceiverHoldsMounts`] refuses first.
    #[error(
        "the invite advertises container `{container}`, which is not the store's root container; \
         pairing adopts the root container only"
    )]
    InviteHasNonRootContainer { container: String },

    /// The invite names a container this device has no document for.
    #[error(
        "the invite advertises container `{container}`, which this device has no document for; \
         pairing adopts a store, it cannot mint containers"
    )]
    InviteContainerUnknown { container: String },
}

/// The first few ids plus a count, so a refusal message stays readable on a
/// store with thousands of blocks.
fn preview(ids: &[String]) -> String {
    const SHOWN: usize = 10;
    if ids.len() <= SHOWN {
        return ids.join(", ");
    }
    format!(
        "{}, … and {} more",
        ids[..SHOWN].join(", "),
        ids.len() - SHOWN
    )
}

/// Property naming the id whose owner-side node this block's content diverged
/// from. Not `_`-prefixed: keys that are are erased on org write-back.
pub const CONFLICT_OF_PROPERTY: &str = "pairing_conflict_of";

/// Where the pre-pair content of a diverged id is kept. Derived from the id, so
/// re-running the re-import after an interrupted pair finds its own copy
/// already there instead of minting a second one.
fn conflict_copy_id(id: &str) -> String {
    format!("{id}-before-pairing")
}

/// This device's version of a block the owner also holds, as a child of the
/// owner's node. The content is carried verbatim — a pair never edits what the
/// user wrote — and the id it diverged from is a property.
fn conflict_copy(snap: &holon_api::SnapshotBlock, id: &str) -> BlockCreateRequest {
    let mut block = snap.block.clone();
    block.id = holon_api::EntityUri::parse(&conflict_copy_id(id))
        .unwrap_or_else(|e| panic!("a conflict copy id derived from {id:?} is not a URI: {e}"));
    let parent = snap.block.id.clone();
    let mut request = BlockCreateRequest::of(&block, &parent);
    request.properties.insert(
        CONFLICT_OF_PROPERTY.to_string(),
        holon_api::Value::String(id.to_string()),
    );
    request
}

/// What one re-import wrote.
#[derive(Debug, Default)]
struct Reimport {
    blocks: usize,
    /// Ids whose content differed from the owner's, kept as conflict copies.
    divergent: Vec<String>,
}

/// The creates one re-import owes the adopted store.
#[derive(Debug, Default)]
struct ReimportPlan {
    requests: Vec<BlockCreateRequest>,
    divergent: Vec<String>,
}

/// Decide what re-importing `own` into a store that already holds `adopted`
/// writes.
///
/// One rule, applied uniformly: a captured id the adopted store ALREADY holds
/// is not re-created, and everything that hung under it hangs under the owner's
/// node of that id instead. That rule is the whole re-parent table — the
/// journals machinery, and a day block whose date the owner also has, are
/// simply ids the owner already holds; a day block the owner lacks is an id it
/// does not, and is re-created with its own id under `block:journals`. Nothing
/// is date-parsed and nothing is special-cased.
///
/// Nothing here reads a store, so re-running it over its own result plans
/// nothing: every id it wrote is then in `adopted`.
fn plan_reimport(
    own: &[(String, holon_api::SnapshotBlock)],
    adopted: &std::collections::HashMap<String, holon_api::SnapshotBlock>,
) -> anyhow::Result<ReimportPlan> {
    let mut placed: std::collections::HashSet<String> = adopted.keys().cloned().collect();
    let mut plan = ReimportPlan::default();
    // `own` is parent-before-child already, but a parent whose own parent the
    // owner holds can precede a sibling subtree; grow to a fixed point rather
    // than assuming one pass reaches every leaf.
    loop {
        let before = plan.requests.len();
        for (id, snap) in own {
            if let Some(theirs) = adopted.get(id) {
                if theirs.block.content != snap.block.content
                    && !plan.divergent.contains(id)
                    && !adopted.contains_key(&conflict_copy_id(id))
                {
                    plan.divergent.push(id.clone());
                    plan.requests.push(conflict_copy(snap, id));
                }
                continue;
            }
            if placed.contains(id) || !placed.contains(snap.block.parent_id.as_str()) {
                continue;
            }
            placed.insert(id.clone());
            plan.requests
                .push(BlockCreateRequest::of(&snap.block, &snap.block.parent_id));
        }
        if plan.requests.len() == before {
            break;
        }
    }

    let orphans: Vec<String> = own
        .iter()
        .filter(|(id, _)| !placed.contains(id.as_str()))
        .map(|(id, snap)| format!("{id} (parent {})", snap.block.parent_id))
        .collect();
    if !orphans.is_empty() {
        return Err(PairingRefused::ReimportHasNoParent { orphans }.into());
    }
    Ok(plan)
}

/// How an invite may be named in a log or a failure message. The invite itself
/// is a bearer credential over every container for its whole TTL, and its
/// tickets carry `CapabilitySecret`s in plaintext.
pub fn invite_fingerprint(invite: &str) -> String {
    let digest = blake3::hash(invite.as_bytes()).to_hex();
    format!("invite {} bytes, blake3 {}", invite.len(), &digest[..16])
}

/// One container's dial coordinates plus the capability the pair is granted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingInvite {
    pub v: u32,
    pub capability: PairCapability,
    /// One ticket per container in the owner's replication set.
    pub containers: Vec<Ticket>,
}

/// Bumped when the shape changes; a mismatched invite is refused on decode.
pub const INVITE_VERSION: u32 = 1;

impl PairingInvite {
    pub fn encode(&self) -> anyhow::Result<String> {
        let json = serde_json::to_vec(self).context("serialize pairing invite")?;
        Ok(URL_SAFE_NO_PAD.encode(json))
    }

    pub fn decode(s: &str) -> anyhow::Result<Self> {
        let bytes = URL_SAFE_NO_PAD
            .decode(s.trim())
            .context("pairing invite is not valid base64 (url-safe, no padding)")?;
        let invite: Self =
            serde_json::from_slice(&bytes).context("pairing invite JSON did not match schema")?;
        if invite.v != INVITE_VERSION {
            bail!(
                "unsupported pairing invite version {} (this build supports v{})",
                invite.v,
                INVITE_VERSION
            );
        }
        if invite.containers.is_empty() {
            bail!("pairing invite advertises no containers — there is nothing to adopt");
        }
        if invite.containers.len() > MAX_INVITE_CONTAINERS {
            bail!(
                "pairing invite advertises {} containers, more than the {MAX_INVITE_CONTAINERS} a \
                 store can hold",
                invite.containers.len()
            );
        }
        Ok(invite)
    }
}

/// Operations for pairing a second device onto this user's whole store.
#[holon_macros::operations_trait]
#[async_trait]
pub trait DevicePairingOperations<T>: MaybeSendSync
where
    T: MaybeSendSync + 'static,
{
    /// Advertise every container in this device's replication set and return a
    /// base64 pairing invite via `OperationResult::response` under `invite`.
    ///
    /// `capability` is `"read"` or `"write"`; `"read"` is refused (see the
    /// module docs). Refuses while another offer is live.
    #[holon_macros::affects("parent_id")]
    async fn pair_offer(&self, capability: String) -> Result<OperationResult>;

    /// Withdraw the live offer: stop advertising its containers and drop the
    /// rosters, so its invite no longer enrolls anyone. The only revocation an
    /// offer has before its TTL.
    #[holon_macros::affects("parent_id")]
    async fn pair_cancel(&self) -> Result<OperationResult>;

    /// Adopt the store the invite advertises. Refuses — before any wire I/O —
    /// a device that holds mounts or whose store is not empty.
    #[holon_macros::affects("parent_id")]
    async fn pair_with_owner(&self, invite: String) -> Result<OperationResult>;
}

/// What one `pair_offer` put on the wire, held so `pair_cancel` can take it all
/// back. The rosters are kept alive because `start_share_gated` binds a roster
/// at advertise time: dropping one would leave an advertised container nobody
/// can enroll into.
struct LiveOffer {
    containers: Vec<String>,
    rosters: Vec<SharedRoster>,
}

/// Resolves the SHARED block-ordering authority the re-import writes through.
///
/// Lazy, and a closure rather than the value: this provider is a member of the
/// set the operation dispatcher is built from, so resolving the ordering at
/// wiring time would be a cycle. Building one here instead would be a second,
/// Loro-blind writer.
pub type OrderingResolver = std::sync::Arc<
    dyn Fn() -> futures::future::BoxFuture<'static, std::sync::Arc<dyn BlockOrdering>>
        + Send
        + Sync,
>;

/// Resolves the downstream SQL projection, lazily and for the same reason as
/// [`OrderingResolver`].
pub type ProjectionResolver = std::sync::Arc<
    dyn Fn() -> futures::future::BoxFuture<
            'static,
            std::sync::Arc<dyn holon_core::DownstreamProjection>,
        > + Send
        + Sync,
>;

/// The pairing operations' production home.
pub struct DevicePairing {
    store: LoroDocumentStore,
    advertiser: std::sync::Arc<IrohAdvertiser>,
    offer: tokio::sync::Mutex<Option<LiveOffer>>,
    ordering: OrderingResolver,
    projection: ProjectionResolver,
    bus: std::sync::Arc<DegradedSignalBus>,
}

impl DevicePairing {
    pub fn new(
        store: LoroDocumentStore,
        advertiser: std::sync::Arc<IrohAdvertiser>,
        ordering: OrderingResolver,
        projection: ProjectionResolver,
        bus: std::sync::Arc<DegradedSignalBus>,
    ) -> Self {
        Self {
            store,
            advertiser,
            offer: tokio::sync::Mutex::new(None),
            ordering,
            projection,
            bus,
        }
    }

    /// Drive the SQL projection to the tree's current state.
    ///
    /// Called BETWEEN the wipe and the adoption, and again after the re-import,
    /// so each of the three steps reaches SQL as its own batch. Interleaved,
    /// they do not survive the trip: the wipe's delete of `block:journals` and
    /// the adoption's re-create of the same stable id land in one batch as a
    /// bare delete of a row the batch's own creates still parent to, and the
    /// deferred foreign key rejects the whole commit.
    async fn flush_projection(&self, stage: &str) -> anyhow::Result<()> {
        // A pass that withholds FK-ungrounded ops still owes them to SQL, and
        // only another pass can pay. Bounded, so a projection that can never
        // ground its ops reports that instead of spinning.
        const PASSES: usize = 8;
        let projection = (self.projection)().await;
        for _ in 0..PASSES {
            let pass = projection.flush().await.map_err(|e| {
                anyhow::anyhow!("projecting the {stage} of this pair into SQL: {e}")
            })?;
            if pass.withheld() == 0 {
                return Ok(());
            }
        }
        bail!(
            "the {stage} of this pair did not reach SQL in {PASSES} projection passes; the block              store the UI reads is behind the store this device adopted"
        )
    }

    /// Stop advertising `containers`, returning the ones that could not be
    /// stopped. A container that stays advertised holds a live capability with
    /// no way left to withdraw it, so the caller must surface the list.
    async fn stop_advertising(&self, containers: &[String]) -> Vec<String> {
        let mut stranded = Vec::new();
        for id in containers {
            if let Err(e) = self.advertiser.drop_share(id).await {
                stranded.push(format!("{id} ({e:#})"));
            }
        }
        stranded
    }

    fn registry(&self) -> ContainerRegistry {
        ContainerRegistry::new(self.store.clone())
    }

    /// Every mount node's BLOCK id, read off the global tree — the same walk
    /// `rehydrate_shared_trees` does, which is the authoritative one. The block
    /// id, not the shared-tree id, because that is what `tree.unshare` takes.
    async fn mounted_share_ids(&self) -> anyhow::Result<Vec<String>> {
        let doc = self.store.get_doc(DocScope::Global).await?;
        doc.with_read(|d| {
            let tree = d.get_tree(crate::loro_backend::TREE_NAME);
            let by_tid = crate::loro_backend::build_tid_index(d);
            let mut out: Vec<String> = tree
                .get_nodes(false)
                .into_iter()
                .filter(|node| crate::shared_tree::is_mount_node(&tree, node.id))
                .filter_map(|node| by_tid.get(&node.id).cloned())
                .collect();
            out.sort();
            Ok(out)
        })
    }

    /// Every live block that is NOT app-seeded — this device's own content,
    /// captured whole so the re-import can rebuild it.
    ///
    /// Both devices seed the same fixed-id families at boot, so a device's own
    /// content is what is left after removing them. App-seeded means exactly
    /// one of: [`LAYOUT_ROOT`] or one of the [`bundled_layout_ids`]; one of the
    /// [`JOURNALS_MACHINERY`] ids; or a journal day block with no children. A
    /// note typed into a day block, and a page created under the layout root,
    /// are the user's.
    ///
    /// Returned in PARENT-BEFORE-CHILD order, siblings by their stored sort
    /// key, because that is the order [`BlockOrdering::create_in_tree_batch`]
    /// creates in and the order sibling positions come out in.
    ///
    /// Read from `doc` rather than from the live store: after the swap the live
    /// store is the owner's, and the content to re-import is in the archived
    /// document.
    fn own_content(
        doc: &crate::LoroDocument,
    ) -> anyhow::Result<Vec<(String, holon_api::SnapshotBlock)>> {
        doc.with_read(|d| {
            let blocks = crate::loro_backend::snapshot_blocks_from_doc(d);

            let mut seeded: std::collections::HashSet<String> =
                std::iter::once(LAYOUT_ROOT.to_string())
                    .chain(bundled_layout_ids())
                    .chain(JOURNALS_MACHINERY.iter().map(|id| (*id).to_string()))
                    .collect();

            let parents: std::collections::HashSet<&str> = blocks
                .values()
                .map(|snap| snap.block.parent_id.as_str())
                .collect();
            let empty_days: Vec<String> = blocks
                .iter()
                .filter(|(id, snap)| {
                    snap.block.parent_id.as_str() == JOURNALS_BLOCK_ID
                        && !seeded.contains(id.as_str())
                        && !parents.contains(id.as_str())
                })
                .map(|(id, _)| id.clone())
                .collect();
            seeded.extend(empty_days);

            let mut own: Vec<(String, holon_api::SnapshotBlock)> = blocks
                .into_iter()
                .filter(|(id, _)| !seeded.contains(id))
                .collect();
            own.sort_by(|(a_id, a), (b_id, b)| {
                (&a.block.parent_id, &a.sort_key, a_id).cmp(&(
                    &b.block.parent_id,
                    &b.sort_key,
                    b_id,
                ))
            });
            Ok(own)
        })
    }

    /// Fetch the owner's global document into `<store>/staging-<stamp>/`,
    /// returning that directory.
    ///
    /// Nothing this device holds is touched: the dial imports into a document
    /// the staging store owns, and the staging store is written to its own
    /// directory. A failure here — an unreachable owner, a refused enrollment,
    /// a stale invite — leaves the device exactly as it was.
    async fn stage_owner_documents(
        &self,
        invite: &PairingInvite,
        stamp: &str,
    ) -> anyhow::Result<(std::path::PathBuf, std::sync::Arc<crate::LoroDocument>)> {
        let dir = self.store.storage_dir().join(format!("staging-{stamp}"));
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating the pairing staging directory {}", dir.display()))?;

        // A bare document, not one the store hands out: the owner's history is
        // replayed into the live document afterwards, and a staged document
        // that had initialized its own schema would replay a third peer's
        // `enable_fractional_index` and `_meta` writes alongside it. Its own
        // peer id for the same reason — two documents minting different ops
        // under one peer id put two histories at the same coordinates.
        let staged = std::sync::Arc::new(crate::LoroDocument::new_with_peer_id(
            crate::loro_document_store::GLOBAL_DOC_ID.to_string(),
            Some(rand::random::<u64>()),
        )?);

        // One endpoint for the whole pair: the acceptor's roster pins peers by
        // QUIC fingerprint, so an endpoint per container would enroll a new
        // "device" for each and exhaust the peer cap.
        let alpns = invite
            .containers
            .iter()
            .map(|t| t.alpn.as_bytes().to_vec())
            .collect();
        let endpoint = create_endpoint(alpns)
            .await
            .context("creating this device's iroh endpoint for pairing")?;

        for ticket in &invite.containers {
            // ALLOW(loro_doc_escape): the iroh sync adapter owns the document
            // for the length of the dial and imports on its own thread.
            let doc = staged.doc();
            tokio::time::timeout(
                DIAL_BUDGET,
                sync_doc_initiate_enrolled(
                    &endpoint,
                    &doc,
                    ticket.alpn.as_bytes(),
                    ticket.addr.clone(),
                    &ticket.capability,
                    &ticket.shared_tree_id,
                ),
            )
            .await
            .with_context(|| {
                format!(
                    "dialing container `{}` did not finish within {DIAL_BUDGET:?}",
                    ticket.shared_tree_id
                )
            })?
            .with_context(|| format!("pairing dial of container `{}`", ticket.shared_tree_id))?;
        }

        staged
            .save_to_file(&crate::pairing_swap::global_snapshot(&dir))
            .context("writing the owner's document to the pairing staging directory")?;
        Ok((dir, staged))
    }

    /// Empty the live global document and replay the owner's history into it.
    ///
    /// The live `LoroDocument` is shared with every writer this process built,
    /// so the adoption changes what it holds instead of handing out a new one.
    /// The owner's history arrives as UPDATES, the form the dial delivers:
    /// importing a snapshot over the wipe's tombstones leaves nodes whose
    /// parent no longer lists them.
    async fn adopt_staged_document(&self, staged: &crate::LoroDocument) -> anyhow::Result<()> {
        let updates = staged.with_read(|d| {
            d.export(loro::ExportMode::all_updates())
                .map_err(|e| anyhow::anyhow!("exporting the owner's history: {e}"))
        })?;
        self.wipe_global_tree().await?;
        self.flush_projection("wipe").await?;

        let doc = self.store.get_doc(DocScope::Global).await?;
        doc.with_write(|txn| {
            // ALLOW(loro_doc_escape): the import runs under the held write
            // guard, on the transaction's own document.
            txn.doc()
                .import(&updates)
                .map_err(|e| anyhow::anyhow!("importing the owner's history: {e}"))?;
            Ok(())
        })?;
        self.flush_projection("adopted store").await
    }

    /// Delete every live node from the global tree, so the owner's history
    /// lands in an empty one.
    ///
    /// This is what makes the pair an ADOPTION rather than a CRDT merge: both
    /// devices minted their own node for `block:journals` and for today's day
    /// block, and importing the owner's history over a tree that still holds
    /// this device's mintings leaves two live nodes under each of those ids.
    /// The nodes deleted here were captured by [`Self::own_content`] first and
    /// are re-created afterwards under the owner's ids.
    async fn wipe_global_tree(&self) -> anyhow::Result<()> {
        let doc = self.store.get_doc(DocScope::Global).await?;
        doc.with_write(|txn| {
            let tree = txn.doc().get_tree(crate::loro_backend::TREE_NAME);
            // Deepest first, and EVERY node explicitly. Deleting a root deletes
            // its subtree in the tree, but the downstream projection retracts
            // by the ids the delete NAMES: an implicit subtree delete leaves
            // the children's SQL rows behind, parented to a row that is gone.
            let mut depth: Vec<(usize, loro::TreeID)> = Vec::new();
            for node in tree.get_nodes(false) {
                if matches!(
                    node.parent,
                    loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
                ) {
                    continue;
                }
                let mut d = 0usize;
                let mut at = node.id;
                while let Some(loro::TreeParentId::Node(parent)) = tree.parent(at) {
                    d += 1;
                    at = parent;
                }
                depth.push((d, node.id));
            }
            depth.sort_by(|a, b| b.0.cmp(&a.0));
            for (_, id) in depth {
                tree.delete(id)
                    .with_context(|| format!("wiping pre-pair tree node {id:?}"))?;
            }
            Ok(())
        })
    }

    /// Every live block in the global tree, by stable id.
    async fn live_blocks(
        &self,
    ) -> anyhow::Result<std::collections::HashMap<String, holon_api::SnapshotBlock>> {
        let doc = self.store.get_doc(DocScope::Global).await?;
        doc.with_read(|d| Ok(crate::loro_backend::snapshot_blocks_from_doc(d)))
    }

    /// Write [`plan_reimport`]'s creates through the shared [`BlockOrdering`] —
    /// the same seam org ingest uses — so the re-imported blocks reach the
    /// projection and the SQL block store like any other create, and keep their
    /// uuids ([`BlockCreateRequest`] carries the id verbatim).
    async fn reimport(
        &self,
        own: &[(String, holon_api::SnapshotBlock)],
    ) -> anyhow::Result<Reimport> {
        let plan = plan_reimport(own, &self.live_blocks().await?)?;
        if plan.requests.is_empty() {
            return Ok(Reimport::default());
        }
        (self.ordering)()
            .await
            .create_in_tree_batch(&plan.requests)
            .await
            .map_err(|e| {
                anyhow::anyhow!("re-importing this device's content after pairing: {e}")
            })?;
        Ok(Reimport {
            blocks: plan.requests.len(),
            divergent: plan.divergent,
        })
    }

    /// Fail loud when an id names more than one live node.
    ///
    /// The pair's postcondition, checked where a merge could have produced the
    /// state rather than left for a reader downstream to trip over: an id with
    /// two live nodes serves two different blocks to everything that resolves
    /// by id.
    async fn assert_one_node_per_id(&self) -> anyhow::Result<()> {
        let doc = self.store.get_doc(DocScope::Global).await?;
        let duplicates: Vec<String> = doc.with_read(|d| {
            let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
            for id in crate::loro_backend::build_tid_index(d).into_values() {
                *counts.entry(id).or_default() += 1;
            }
            Ok(counts
                .into_iter()
                .filter(|(_, n)| *n > 1)
                .map(|(id, n)| format!("{id} ×{n}"))
                .collect())
        })?;
        if duplicates.is_empty() {
            return Ok(());
        }
        Err(PairingRefused::PairLeftDuplicateNodes { duplicates }.into())
    }

    /// Every reason this device cannot adopt this invite, decided before the
    /// store is touched and before any dial.
    async fn refuse_unpairable(&self, invite: &PairingInvite) -> anyhow::Result<()> {
        let mounts = self.mounted_share_ids().await?;
        if !mounts.is_empty() {
            return Err(PairingRefused::ReceiverHoldsMounts { mounts }.into());
        }
        if let Some(record) = crate::pairing_swap::read_record(self.store.storage_dir())? {
            return Err(PairingRefused::AlreadyPaired {
                owner: record.owner,
                paired_at: record.paired_at,
                archive: record.archive.display().to_string(),
            }
            .into());
        }
        let set = self.registry().replication_set().await?;
        for ticket in &invite.containers {
            if !set.iter().any(|c| c.id == ticket.shared_tree_id) {
                return Err(PairingRefused::InviteContainerUnknown {
                    container: ticket.shared_tree_id.clone(),
                }
                .into());
            }
            if ticket.shared_tree_id != crate::container_registry::ROOT_CONTAINER_ID {
                return Err(PairingRefused::InviteHasNonRootContainer {
                    container: ticket.shared_tree_id.clone(),
                }
                .into());
            }
        }
        Ok(())
    }

    /// Re-import the content this device wrote before the swap, read from the
    /// archived document the swap moved aside.
    ///
    /// Idempotent by id, so a pair interrupted mid-re-import is finished by
    /// running this again: a block the live store already holds is not
    /// re-created, and a conflict copy's id is derived from the id it diverged
    /// from.
    async fn reimport_from_archive(&self, archive: &std::path::Path) -> anyhow::Result<Reimport> {
        let path = crate::pairing_swap::global_snapshot(archive);
        let archived = crate::LoroDocument::load_from_file(&path, "holon_tree_archive".to_string())
            .with_context(|| {
                format!(
                    "opening the pre-pair document {} to re-import from",
                    path.display()
                )
            })?;
        let own = Self::own_content(&archived)?;
        let reimported = self.reimport(&own).await?;
        self.flush_projection("re-import").await?;
        self.assert_one_node_per_id().await?;
        Ok(reimported)
    }

    /// Tell the user what this device contributed to the store it adopted and
    /// where its pre-pair document is. The archive path is the only handle on
    /// content the re-import could not carry.
    fn disclose(&self, archive: &std::path::Path, reimported: &Reimport) {
        if reimported.blocks == 0 {
            return;
        }
        self.bus.emit(ShareDegraded {
            shared_tree_id: DEVICE_ENTITY.to_string(),
            reason: ShareDegradedReason::PairingReimportedLocalContent {
                blocks: reimported.blocks,
                conflict_copies: reimported.divergent.len(),
                archive: archive.display().to_string(),
            },
        });
    }

    /// Finish a pair whose re-import did not run, or did not finish.
    ///
    /// Called at boot after [`crate::pairing_swap::complete_interrupted_swap`]
    /// has put the owner's document in place: the store is the owner's and the
    /// content this device wrote is still only in the archive.
    pub async fn complete_interrupted_pairing(
        &self,
        marker: &crate::pairing_swap::PairingMarker,
    ) -> anyhow::Result<Reimported> {
        let store_dir = self.store.storage_dir().to_path_buf();
        let reimported = self.reimport_from_archive(&marker.archive).await?;
        self.store
            .save_all()
            .await
            .context("flushing the store an interrupted pair left behind")?;
        crate::pairing_swap::write_record(
            &store_dir,
            &crate::pairing_swap::PairingRecord {
                owner: marker.owner.clone(),
                paired_at: marker.started_at.clone(),
                containers: vec![crate::container_registry::ROOT_CONTAINER_ID.to_string()],
                archive: marker.archive.clone(),
            },
        )?;
        crate::pairing_swap::remove_marker(&store_dir)?;
        self.disclose(&marker.archive, &reimported);
        Ok(Reimported {
            blocks: reimported.blocks,
            conflict_copies: reimported.divergent.len(),
        })
    }
}

/// What finishing an interrupted pair wrote.
#[derive(Debug)]
pub struct Reimported {
    pub blocks: usize,
    pub conflict_copies: usize,
}

#[async_trait]
impl DevicePairingOperations<()> for DevicePairing {
    async fn pair_offer(&self, capability: String) -> Result<OperationResult> {
        let capability = PairCapability::parse(&capability)?;
        if capability == PairCapability::Read {
            return Err(PairingRefused::ReadOnlyPairingUnsupported.into());
        }
        let mut live = self.offer.lock().await;
        if let Some(offer) = live.as_ref() {
            return Err(PairingRefused::OfferAlreadyLive {
                containers: offer.containers.len(),
            }
            .into());
        }
        let expires_at = ExpiryTime(chrono::Utc::now().timestamp() + INVITE_TTL_SECS);

        // One roster per container: a `ShareRoster` binds ONE `shared_tree_id`
        // and the enrollment proof is bound to it, so a single roster could
        // never gate a second container.
        let set = self.registry().replication_set().await?;
        let mut containers = Vec::with_capacity(set.len());
        let mut minted = LiveOffer {
            containers: Vec::with_capacity(set.len()),
            rosters: Vec::with_capacity(set.len()),
        };
        for container in &set {
            let secret = CapabilitySecret::generate();
            let roster: SharedRoster =
                std::sync::Arc::new(tokio::sync::Mutex::new(ShareRoster::new(
                    container.id.clone(),
                    secret.clone(),
                    expires_at,
                    INVITE_MAX_PEERS,
                )));
            let advertised = self
                .advertiser
                .start_share_gated(
                    container.id.clone(),
                    // ALLOW(loro_doc_escape): the advertiser retains the
                    // document for the life of the share and serves dials from
                    // its own task — a long-lived transport site.
                    container.doc.doc(),
                    roster.clone(),
                    None,
                    None,
                )
                .await;
            let addr = match advertised {
                Ok(addr) => addr,
                Err(e) => {
                    let stranded = self.stop_advertising(&minted.containers).await;
                    let mut msg = format!(
                        "advertising container `{}` for pairing: {e:#}",
                        container.id
                    );
                    if !stranded.is_empty() {
                        msg.push_str(&format!(
                            "; and these containers stayed advertised under a capability no \
                             invite names: {}",
                            stranded.join(", ")
                        ));
                    }
                    return Err(anyhow::anyhow!(msg).into());
                }
            };
            minted.containers.push(container.id.clone());
            minted.rosters.push(roster);
            containers.push(Ticket::new(
                container.id.clone(),
                addr,
                String::from_utf8(make_alpn(ALPN_PREFIX, &container.id))
                    .context("container ALPN is not UTF-8")?,
                secret,
                expires_at,
            ));
        }

        let invite = PairingInvite {
            v: INVITE_VERSION,
            capability,
            containers,
        }
        .encode()?;
        *live = Some(minted);

        Ok(OperationResult::declared_irreversible(
            vec![],
            "device.pair_offer: an invite cannot be un-minted; withdraw it with device.pair_cancel",
        )
        .with_response(Value::String(
            serde_json::json!({ "invite": invite, "containers": set.len() }).to_string(),
        )))
    }

    async fn pair_cancel(&self) -> Result<OperationResult> {
        let Some(offer) = self.offer.lock().await.take() else {
            return Err(PairingRefused::NoLiveOffer.into());
        };
        let count = offer.containers.len();
        let stranded = self.stop_advertising(&offer.containers).await;
        if !stranded.is_empty() {
            return Err(anyhow::anyhow!(
                "the offer's rosters are gone but these containers are still advertised, so this \
                 process keeps serving them until it exits: {}",
                stranded.join(", ")
            )
            .into());
        }

        Ok(OperationResult::declared_irreversible(
            vec![],
            "device.pair_cancel: a withdrawn offer cannot be re-issued, only replaced",
        )
        .with_response(Value::String(
            serde_json::json!({ "cancelled_containers": count }).to_string(),
        )))
    }

    async fn pair_with_owner(&self, invite: String) -> Result<OperationResult> {
        let invite = PairingInvite::decode(&invite)?;
        self.refuse_unpairable(&invite).await?;

        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ").to_string();
        let (staging, staged) = self.stage_owner_documents(&invite, &stamp).await?;

        // From here the live store changes. The marker names both directories,
        // so a process killed at any point between the two renames is finished
        // by `complete_interrupted_swap` at the next boot instead of opening a
        // store with no global document.
        let store_dir = self.store.storage_dir().to_path_buf();
        let marker = crate::pairing_swap::PairingMarker {
            archive: store_dir.join("archive").join(&stamp),
            staging,
            owner: owner_endpoint(&invite),
            started_at: stamp.clone(),
        };
        crate::pairing_swap::write_marker(&store_dir, &marker)?;

        self.store
            .save_all()
            .await
            .context("flushing this device's documents before archiving them")?;
        crate::pairing_swap::archive_global(&store_dir, &stamp)?;
        crate::pairing_swap::promote_staged(&store_dir, &marker.staging)?;

        self.adopt_staged_document(&staged).await?;
        let reimported = self.reimport_from_archive(&marker.archive).await?;

        self.store
            .save_all()
            .await
            .context("flushing the adopted store to disk")?;
        crate::pairing_swap::write_record(
            &store_dir,
            &crate::pairing_swap::PairingRecord {
                owner: marker.owner.clone(),
                paired_at: stamp,
                containers: invite
                    .containers
                    .iter()
                    .map(|t| t.shared_tree_id.clone())
                    .collect(),
                archive: marker.archive.clone(),
            },
        )?;
        crate::pairing_swap::remove_marker(&store_dir)?;

        self.disclose(&marker.archive, &reimported);

        Ok(OperationResult::declared_irreversible(
            vec![],
            "device.pair_with_owner: an adopted store cannot be un-adopted",
        )
        .with_response(Value::String(
            serde_json::json!({
                "containers": invite.containers.len(),
                "reimported_blocks": reimported.blocks,
                "conflict_copies": reimported.divergent.len(),
            })
            .to_string(),
        )))
    }
}

/// The owner's endpoint, as the invite advertises it. Every ticket a validated
/// invite carries names the root container, so any of them names the owner.
fn owner_endpoint(invite: &PairingInvite) -> String {
    invite
        .containers
        .first()
        .expect("a decoded invite carries at least one container")
        .addr
        .id
        .to_string()
}

#[async_trait]
impl OperationProvider for DevicePairing {
    fn operations(&self) -> Vec<OperationDescriptor> {
        __operations_device_pairing_operations::device_pairing_operations(
            DEVICE_ENTITY,
            DEVICE_ENTITY,
            DEVICE_ENTITY,
            "id",
        )
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        if entity_name != DEVICE_ENTITY {
            return Err(format!(
                "DevicePairing expects entity '{DEVICE_ENTITY}', got '{entity_name}'"
            )
            .into());
        }
        __operations_device_pairing_operations::dispatch_operation::<_, ()>(self, op_name, &params)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(id: &str, parent: &str, content: &str) -> (String, holon_api::SnapshotBlock) {
        let mut block = holon_api::block::Block {
            id: holon_api::EntityUri::parse(id).expect("a block uri"),
            parent_id: holon_api::EntityUri::parse(parent).expect("a parent uri"),
            ..Default::default()
        };
        block.content = content.to_string();
        (
            id.to_string(),
            holon_api::SnapshotBlock {
                block,
                sort_key: "a0".to_string(),
            },
        )
    }

    fn store_of(
        blocks: &[(String, holon_api::SnapshotBlock)],
    ) -> std::collections::HashMap<String, holon_api::SnapshotBlock> {
        blocks.iter().cloned().collect()
    }

    #[test]
    fn a_block_the_owner_also_holds_with_different_content_is_kept_under_the_owners_node() {
        let day = snap("block:day", "block:journals", "Sunday — groceries");
        let owner = store_of(&[
            snap("block:journals", "block:root", "Journals"),
            snap("block:day", "block:journals", "Sunday"),
        ]);
        let plan = plan_reimport(&[day.clone()], &owner).expect("a plan");

        assert_eq!(plan.divergent, vec!["block:day".to_string()]);
        let copy = plan
            .requests
            .iter()
            .find(|r| r.id.as_str() == "block:day-before-pairing")
            .expect("the phone's version is kept as its own block");
        assert_eq!(copy.parent_id.as_str(), "block:day");
        assert_eq!(
            copy.properties.get(CONFLICT_OF_PROPERTY),
            Some(&holon_api::Value::String("block:day".to_string()))
        );
    }

    #[test]
    fn a_block_the_owner_holds_with_the_same_content_is_not_written_again() {
        let day = snap("block:day", "block:journals", "Sunday");
        let owner = store_of(&[
            snap("block:journals", "block:root", "Journals"),
            day.clone(),
        ]);
        let plan = plan_reimport(&[day], &owner).expect("a plan");
        assert!(plan.requests.is_empty() && plan.divergent.is_empty());
    }

    #[test]
    fn a_second_re_import_over_its_own_result_writes_nothing() {
        let own = [
            snap("block:day", "block:journals", "Sunday — groceries"),
            snap("block:note", "block:day", "bought milk"),
        ];
        let mut adopted = store_of(&[
            snap("block:journals", "block:root", "Journals"),
            snap("block:day", "block:journals", "Sunday"),
        ]);
        let first = plan_reimport(&own, &adopted).expect("a first plan");
        assert_eq!(first.requests.len(), 2);

        for request in &first.requests {
            let (id, written) = snap(
                request.id.as_str(),
                request.parent_id.as_str(),
                own.iter()
                    .find(|(candidate, _)| candidate == request.id.as_str())
                    .map(|(_, s)| s.block.content.as_str())
                    .unwrap_or("Sunday — groceries"),
            );
            adopted.insert(id, written);
        }

        let second = plan_reimport(&own, &adopted).expect("a second plan");
        assert!(
            second.requests.is_empty(),
            "finishing an interrupted re-import wrote {} block(s) a second time: {:?}",
            second.requests.len(),
            second
                .requests
                .iter()
                .map(|r| r.id.to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_block_whose_parent_neither_side_holds_is_refused_by_name() {
        let orphan = snap("block:note", "block:vanished", "bought milk");
        let owner = store_of(&[snap("block:journals", "block:root", "Journals")]);
        let err = plan_reimport(&[orphan], &owner).expect_err("an orphan has nowhere to go");
        let message = err.to_string();
        assert!(
            message.contains("block:note") && message.contains("block:vanished"),
            "the refusal must name the block and the parent it wanted; got: {message}"
        );
    }

    #[test]
    fn capability_parses_only_the_two_words() {
        assert_eq!(PairCapability::parse("read").unwrap(), PairCapability::Read);
        assert_eq!(
            PairCapability::parse("write").unwrap(),
            PairCapability::Write
        );
        assert!(PairCapability::parse("readwrite").is_err());
        assert!(PairCapability::parse("").is_err());
    }

    #[test]
    fn an_invite_with_no_containers_is_refused() {
        let empty = PairingInvite {
            v: INVITE_VERSION,
            capability: PairCapability::Write,
            containers: Vec::new(),
        }
        .encode()
        .unwrap();
        let err = PairingInvite::decode(&empty).unwrap_err();
        assert!(
            err.to_string().contains("no containers"),
            "an empty invite must say so; got: {err}"
        );
    }

    #[test]
    fn an_invite_with_too_many_containers_is_refused() {
        let ticket = || {
            Ticket::new(
                "tree".into(),
                iroh::EndpointAddr::new(iroh::SecretKey::generate(&mut rand::rng()).public()),
                "loro-sync/tree".into(),
                CapabilitySecret::generate(),
                ExpiryTime(1_000_000),
            )
        };
        let oversized = PairingInvite {
            v: INVITE_VERSION,
            capability: PairCapability::Write,
            containers: (0..MAX_INVITE_CONTAINERS + 1).map(|_| ticket()).collect(),
        }
        .encode()
        .unwrap();
        let Err(err) = PairingInvite::decode(&oversized) else {
            panic!(
                "an invite with {} containers decoded",
                MAX_INVITE_CONTAINERS + 1
            );
        };
        assert!(
            err.to_string().contains(&MAX_INVITE_CONTAINERS.to_string()),
            "the refusal must name the cap; got: {err}"
        );
    }

    #[test]
    fn an_invite_enrolls_exactly_one_device() {
        use crate::share_enrollment::AuthzReject;
        use crate::share_enrollment::Challenge;
        use crate::share_enrollment::EnrollmentProofMsg;
        use crate::share_enrollment::PeerFingerprint;

        let secret = CapabilitySecret::generate();
        let mut roster = ShareRoster::new(
            "tree-pair",
            secret.clone(),
            ExpiryTime(10_000),
            INVITE_MAX_PEERS,
        );
        let challenge = Challenge::generate();
        let msg = EnrollmentProofMsg::build(&secret, &challenge, "tree-pair");
        roster
            .authorize(
                100,
                &challenge,
                &msg.capability_id,
                &msg.proof,
                PeerFingerprint::from_bytes([1; 32]),
            )
            .expect("the invited device enrolls");
        let err = roster
            .authorize(
                100,
                &challenge,
                &msg.capability_id,
                &msg.proof,
                PeerFingerprint::from_bytes([2; 32]),
            )
            .expect_err("a pairing invite admits ONE device");
        assert!(matches!(err, AuthzReject::RosterFull { max: 1 }), "{err}");
    }

    #[test]
    fn the_invite_fingerprint_never_quotes_the_invite() {
        let invite = "eyJ2IjoxLCJjYXBhYmlsaXR5Ijoid3JpdGUifQ";
        let named = invite_fingerprint(invite);
        assert!(!named.contains(invite), "{named}");
        assert!(named.contains(&invite.len().to_string()), "{named}");
    }

    #[test]
    fn the_mounts_refusal_names_every_mount() {
        let refusal = PairingRefused::ReceiverHoldsMounts {
            mounts: vec!["tree-a".into(), "tree-b".into()],
        };
        let msg = refusal.to_string();
        assert!(msg.contains("tree-a") && msg.contains("tree-b"), "{msg}");
    }
}
