//! `inv-share-mount-carries-page-identity` — a shared page is ONE row on each
//! peer, under the page's own id, carrying the page's own fields.
//!
//! @pbt oracle model — the sharing overlay's page shares vs. both peers'
//!   `block_raw`, with the owner's row as the field reference.
//! @pbt covers two-instance-page-share — a page share that projects a mount row
//!   under a fresh id, drops the page's own row, or copies only part of the
//!   page's fields onto the mount.
//!
//! The mount node is the receiver's PLACEMENT record (Overlay proposal §2): it
//! decides where the page hangs, never what the page is. So the row a reader
//! sees is the page itself — same id on both peers, the owner's properties and
//! tags, and no row anywhere that names the mount.

use std::collections::BTreeSet;

use holon_api::share_props::SHARE_ROLE_MOUNT;
use holon_api::share_props::SHARE_ROLE_PROPERTY;
use holon_api::share_props::SHARED_TREE_ID_PROPERTY;
use holon_pbt_core::capabilities::RefSharedView;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutReceiverBackend;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvShareMountCarriesPageIdentity;

impl InvShareMountCarriesPageIdentity {
    pub const ID: InvariantId = InvariantId("inv-share-mount-carries-page-identity");
}

fn property_str(block: &holon_api::Block, key: &str) -> Option<String> {
    block
        .properties_map()
        .get(key)
        .and_then(|v| v.as_string().map(str::to_string))
}

/// The ids of `roots` and of every row below them, by parent links within
/// `rows` — the blocks a per-page share of `roots` carries.
pub fn page_share_members<'a>(
    roots: impl IntoIterator<Item = &'a holon_api::EntityUri>,
    rows: &[holon_api::Block],
) -> BTreeSet<holon_api::EntityUri> {
    let mut members: BTreeSet<holon_api::EntityUri> = roots.into_iter().cloned().collect();
    loop {
        let before = members.len();
        for row in rows {
            if members.contains(&row.parent_id) {
                members.insert(row.id.clone());
            }
        }
        if members.len() == before {
            return members;
        }
    }
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvShareMountCarriesPageIdentity
where
    R: RefSharedView,
    S: SutReceiverBackend + SutBackend,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let shares = ref_.page_shares();
        if shares.is_empty() {
            return InvariantResult::Skipped("the model records no per-page share".into());
        }
        let owner = sut.block_raw_snapshot().await;
        let receiver = sut.receiver_block_raw_snapshot().await;
        let owner_ids: BTreeSet<_> = owner.iter().map(|b| b.id.clone()).collect();

        for page in shares.keys() {
            for (side, rows) in [("owner", &owner), ("receiver", &receiver)] {
                let named: Vec<_> = rows.iter().filter(|b| &b.id == page).collect();
                if named.len() != 1 {
                    let mounts: Vec<String> = rows
                        .iter()
                        .filter(|b| {
                            property_str(b, SHARE_ROLE_PROPERTY).as_deref()
                                == Some(SHARE_ROLE_MOUNT)
                        })
                        .map(|b| format!("{} {:?}", b.id, b.content))
                        .collect();
                    return InvariantResult::Fail(format!(
                        "[inv-share-mount-carries-page-identity] the {side} holds {} row(s) with \
                         the shared page's id {page}, expected exactly one. Mount rows it holds \
                         instead: {mounts:?}",
                        named.len()
                    ));
                }
            }
            let owner_row = owner
                .iter()
                .find(|b| &b.id == page)
                .expect("exactly one owner row was just checked");
            let receiver_row = receiver
                .iter()
                .find(|b| &b.id == page)
                .expect("exactly one receiver row was just checked");

            let Some(stid) = property_str(receiver_row, SHARED_TREE_ID_PROPERTY) else {
                return InvariantResult::Fail(format!(
                    "[inv-share-mount-carries-page-identity] the receiver's row {page} carries no \
                     `{SHARED_TREE_ID_PROPERTY}`, so nothing ties it to the share it came from"
                ));
            };

            if receiver_row.content != owner_row.content {
                return InvariantResult::Fail(format!(
                    "[inv-share-mount-carries-page-identity] the receiver's {page} is titled {:?}, \
                     the owner's {:?}",
                    receiver_row.content, owner_row.content
                ));
            }
            if receiver_row.tags != owner_row.tags {
                return InvariantResult::Fail(format!(
                    "[inv-share-mount-carries-page-identity] the receiver's {page} carries tags \
                     {:?}, the owner's {:?}",
                    receiver_row.tags, owner_row.tags
                ));
            }
            let receiver_props = receiver_row.properties_map();
            for (key, value) in owner_row.properties_map() {
                if receiver_props.get(&key) != Some(&value) {
                    return InvariantResult::Fail(format!(
                        "[inv-share-mount-carries-page-identity] the owner's {page} carries \
                         property {key} = {value:?}, the receiver's carries {:?}",
                        receiver_props.get(&key)
                    ));
                }
            }

            for (side, rows) in [("owner", &owner), ("receiver", &receiver)] {
                let members = page_share_members([page], rows);
                if let Some(unstamped) = rows
                    .iter()
                    .filter(|b| members.contains(&b.id))
                    .find(|b| property_str(b, SHARED_TREE_ID_PROPERTY).as_deref() != Some(&stid))
                {
                    return InvariantResult::Fail(format!(
                        "[inv-share-mount-carries-page-identity] the {side}'s {} sits in the share \
                         of {page} but is stamped {:?}, not the share's {stid:?}",
                        unstamped.id,
                        property_str(unstamped, SHARED_TREE_ID_PROPERTY)
                    ));
                }
                let share_rows: Vec<_> = rows
                    .iter()
                    .filter(|b| property_str(b, SHARED_TREE_ID_PROPERTY) == Some(stid.clone()))
                    .collect();
                if let Some(mount) = share_rows.iter().find(|b| {
                    property_str(b, SHARE_ROLE_PROPERTY).as_deref() == Some(SHARE_ROLE_MOUNT)
                }) {
                    return InvariantResult::Fail(format!(
                        "[inv-share-mount-carries-page-identity] the {side} projects the mount of \
                         the share of {page} as its own row {} {:?} — the mount is a placement \
                         record, never a block",
                        mount.id, mount.content
                    ));
                }
                if side == "receiver"
                    && let Some(fresh) = share_rows.iter().find(|b| !owner_ids.contains(&b.id))
                {
                    return InvariantResult::Fail(format!(
                        "[inv-share-mount-carries-page-identity] the receiver holds {} {:?} in the \
                         share of {page}, an id the owner does not hold — a share must carry the \
                         owner's ids, never mint its own",
                        fresh.id, fresh.content
                    ));
                }
            }
        }
        InvariantResult::Ok
    }
}
