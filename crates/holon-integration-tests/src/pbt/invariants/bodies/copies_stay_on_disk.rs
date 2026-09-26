//! `inv-copies-stay-on-disk` — Holon never removes a copy of a block from disk.
//!
//! @pbt oracle model-equivalence — for every block the model says is in two
//!   files, the owner's file holds the block with the model's text, and the
//!   file with the copy holds exactly the subtree the editor last wrote there
//!   (headline levels aside)
//! @pbt covers move-block-between-files — the state between the two saves of
//!   a cut & paste, where both files hold the block, with Holon writing to
//!   both (D229.b)
//! @pbt slips-if-removed an ingest or a write-back renders the file with the
//!   copy from the store and drops or rewrites the copy, or re-homes the
//!   block so the owner's next write-back drops it there

use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefCopies;
use holon_pbt_core::capabilities::SutEditorSaves;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvCopiesStayOnDisk;

impl InvCopiesStayOnDisk {
    pub const ID: InvariantId = InvariantId("inv-copies-stay-on-disk");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvCopiesStayOnDisk
where
    R: RefCopies + RefBlockTree,
    S: SutEditorSaves,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, reference: &R, sut: &S) -> InvariantResult {
        use holon_orgmode::subtree::cut_subtree;
        use holon_orgmode::subtree::relevel;

        let mut failures = Vec::new();
        for copy in reference.model_copies() {
            let bare = &copy.block_bare;
            let owner = sut.on_disk(&copy.owner_doc).await.unwrap_or_default();
            let content = reference
                .block_content(&holon_api::EntityUri::block(bare))
                .unwrap_or_else(|| panic!("the model's copied block {bare} has no content"))
                .to_string();
            match cut_subtree(&owner, bare) {
                Some((_, section)) if section.contains(content.lines().next().unwrap_or("")) => {}
                found => failures.push(format!(
                    "the owner's file {} does not hold {bare} with the text {content:?}: {found:?}",
                    copy.owner_file
                )),
            }
            let pasted = sut
                .pasted_copy(&copy.copy_doc, bare)
                .await
                .unwrap_or_else(|| panic!("the editor pasted no {bare} into {}", copy.copy_file));
            let on_disk = sut
                .on_disk(&copy.copy_doc)
                .await
                .and_then(|disk| cut_subtree(&disk, bare))
                .map(|(_, section)| relevel(&section, 1));
            if on_disk.as_deref() != Some(pasted.as_str()) {
                failures.push(format!(
                    "{} no longer holds the copy of {bare} the editor wrote.\nwritten:\n{pasted}\n\
                     on disk:\n{on_disk:?}",
                    copy.copy_file
                ));
            }
        }
        if failures.is_empty() {
            InvariantResult::Ok
        } else {
            InvariantResult::Fail(failures.join("\n"))
        }
    }
}
