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
use serde::Deserialize;
use serde::Serialize;

use crate::container_registry::ContainerRegistry;
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

    /// The receiver's store already holds USER content — blocks outside the
    /// fixed-id families every device seeds at boot. Pairing adopts the
    /// owner's store, and this device's own content is not in it, so the pair
    /// would silently drop the blocks named here.
    ///
    /// D78.d (archive the receiver's store, re-import it after the pair)
    /// replaces this refusal; `ReceiverNotEmpty` is the whole grep.
    #[error(
        "this device holds {} block(s) of its own that pairing would drop; \
         pair a device that holds no content of its own: {}",
        blocks.len(),
        preview(blocks)
    )]
    ReceiverNotEmpty { blocks: Vec<String> },

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

/// The pairing operations' production home.
pub struct DevicePairing {
    store: LoroDocumentStore,
    advertiser: std::sync::Arc<IrohAdvertiser>,
    offer: tokio::sync::Mutex<Option<LiveOffer>>,
}

impl DevicePairing {
    pub fn new(store: LoroDocumentStore, advertiser: std::sync::Arc<IrohAdvertiser>) -> Self {
        Self {
            store,
            advertiser,
            offer: tokio::sync::Mutex::new(None),
        }
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

    /// Every live block that is NOT app-seeded — this device's own content.
    ///
    /// "Empty" for pairing means "holds no content of its own". Both devices
    /// seed the same fixed-id families at boot, so a freshly booted receiver is
    /// never literally empty and a node-count test would refuse every real
    /// pair. App-seeded means exactly one of: [`LAYOUT_ROOT`] or one of the
    /// [`bundled_layout_ids`]; one of the [`JOURNALS_MACHINERY`] ids; or a
    /// journal day block with no children. A note typed into a day block, and a
    /// page created under the layout root, are the user's.
    async fn blocks_outside_the_app_seeded_families(&self) -> anyhow::Result<Vec<String>> {
        let doc = self.store.get_doc(DocScope::Global).await?;
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

            let mut own: Vec<String> = blocks
                .into_keys()
                .filter(|id| !seeded.contains(id))
                .collect();
            own.sort();
            Ok(own)
        })
    }
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

        // Both preconditions are decided BEFORE any wire I/O, so a refusal
        // leaves this device exactly as it was.
        let mounts = self.mounted_share_ids().await?;
        if !mounts.is_empty() {
            return Err(PairingRefused::ReceiverHoldsMounts { mounts }.into());
        }
        let blocks = self.blocks_outside_the_app_seeded_families().await?;
        if !blocks.is_empty() {
            return Err(PairingRefused::ReceiverNotEmpty { blocks }.into());
        }

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

        let set = self.registry().replication_set().await?;
        let mut adopted = 0usize;
        for ticket in &invite.containers {
            let container = set
                .iter()
                .find(|c| c.id == ticket.shared_tree_id)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "the invite advertises container `{}`, which this device has no document \
                         for; pairing adopts a store, it cannot mint containers",
                        ticket.shared_tree_id
                    )
                })?;
            // ALLOW(loro_doc_escape): the iroh sync adapter owns the document
            // for the length of the dial and imports on its own thread.
            let doc = container.doc.doc();
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
            adopted += 1;
        }

        self.store
            .save_all()
            .await
            .context("flushing the adopted store to disk")?;

        Ok(OperationResult::declared_irreversible(
            vec![],
            "device.pair_with_owner: an adopted store cannot be un-adopted",
        )
        .with_response(Value::String(
            serde_json::json!({ "containers": adopted }).to_string(),
        )))
    }
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
