//! The reference model of a block on disk in two files (D229.b).
//!
//! An external editor pastes a heading into a second file while the file
//! that owns it still holds it. The owner stays authoritative, both files
//! keep their bytes, and the app discloses the pair until the user resolves
//! it: the owner's file lets the block go (the copy is adopted, merged with
//! Holon's edits), the user deletes the copy, Holon deletes the block (the
//! copy brings it back), or the owner's file is deleted (the copy is adopted).
//!
//! The copy's only field the editor edits is its task keyword, so the merge
//! is stated on that one field; everything else the copy holds equals the
//! pasted ancestor, and the store's side wins it.
//!
//! @pbt kind ref
//! @pbt covers move-block-between-files — the copy map, its conditions, and
//!   the rules that end a copy

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use holon_api::EntityUri;
use holon_api::TaskState;
use holon_api::block::Block;
use holon_orgmode::models::OrgBlockExt;
use holon_pbt_core::capabilities::DrawnHome;
use holon_pbt_core::capabilities::ModelCopy;
use holon_pbt_core::capabilities::RefDocuments;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use validated::Validated;

use crate::pbt::file_adapter_state::CopyOnDisk;
use crate::pbt::file_adapter_state::PastedCopy;
use crate::pbt::reference_state::ReferenceState;

/// The condition kinds the model governs in every state, so a spurious one
/// fails the run.
pub const COPY_KINDS: [&str; 5] = [
    holon_api::ConditionKind::BLOCK_IN_TWO_FILES,
    holon_api::ConditionKind::BLOCK_EDITED_IN_TWO_FILES,
    holon_api::ConditionKind::DELETED_BLOCK_KEPT_IN_FILE,
    holon_api::ConditionKind::DELETION_UNDONE_BLOCK_IN_OTHER_FILE,
    holon_api::ConditionKind::DELETION_ENDED_BY_EDIT,
];

/// The transitions the model describes while a copy stands. The others
/// rewrite page files from the model (which would drop a copy), change a
/// file's identity, restart the app (the copy state is in memory), undo
/// across an ingest, or move blocks between pages.
const ALLOWED_WHILE_COPIES_STAND: &[&str] = &[
    "PasteBlockCopy",
    "FinishCutPaste",
    "EditBlockCopy",
    "Reboot",
    "DeleteLineFromFile",
    "ToggleState",
    "TypeChars",
    "SplitBlock",
    "JoinBlock",
    "CreateBlockUnderFocus",
    "Indent",
    "Outdent",
    "MoveUp",
    "MoveDown",
    "ToggleCollapse",
    "CreateDocument",
    "DeleteDocument",
    "ClickBlock",
    "FocusEditableText",
    "MoveCursor",
    "ArrowNavigate",
    "NavigateBack",
    "NavigateForward",
    "NavigateHome",
    "NavigateFocus",
    "SwitchView",
    "SwitchViewMode",
    "ToggleDrawer",
    "ExpandToggle",
    "WheelScroll",
    "Search",
    "JumpToSearchHit",
    "SetupWatch",
    "RemoveWatch",
    "PinBlock",
    "UnpinBlock",
    "OpenTabViaModifierClick",
    "Nothing",
];

/// Each copied heading's owner document and subtree before a transition, so
/// the model can tell what the transition did to it.
pub struct CopiesBefore(BTreeMap<EntityUri, (EntityUri, Vec<Block>)>);

/// The file name the model knows `doc` by, as a condition names it.
pub fn file_name_of(state: &ReferenceState, doc: &EntityUri) -> String {
    let name = state
        .files
        .documents
        .get(doc)
        .unwrap_or_else(|| panic!("{doc} has no file in the model"));
    std::path::Path::new(name)
        .file_name()
        .unwrap_or_else(|| panic!("{name} has no file name"))
        .to_string_lossy()
        .into_owned()
}

fn keyword(block: &Block) -> Option<String> {
    block.task_state().map(|state| state.keyword)
}

/// The copy's task keyword merged with the store's against the pasted one:
/// the side that changed wins. `None` when both changed it apart and the
/// copy does not win conflicts.
fn merged_keyword(
    base: &Option<String>,
    disk: &Option<String>,
    store: &Option<String>,
    disk_wins_conflicts: bool,
) -> Option<Option<String>> {
    if disk == base {
        Some(store.clone())
    } else if store == base || disk == store || disk_wins_conflicts {
        Some(disk.clone())
    } else {
        None
    }
}

fn set_keyword(block: &mut Block, keyword: Option<String>) {
    if keyword == block.task_state().map(|state| state.keyword) {
        return;
    }
    block.set_task_state(keyword.map(|k| TaskState::from_keyword(&k)));
}

impl ReferenceState {
    /// Every transition the model does not describe while a copy stands is
    /// disabled, and a restart while a brought-back block is disclosed (the
    /// disclosure is in memory). After a restart the app no longer knows a
    /// copy's pasted ancestor, and the model does not describe a merge
    /// without it: the owner's file is not deleted then.
    pub fn copies_window_gate(&self, variant: &str) -> Validated<(), Reason> {
        let restart = matches!(
            variant,
            "Reboot"
                | "SimulateRestart"
                | "EpochFlipRejected"
                | "DeleteHeadlineWhileOff"
                | "RebootWithBacklogHeld"
                | "RebootAfterUpgrade"
        );
        let base_forgotten = self.files.copies.values().any(|copy| !copy.base_known);
        check(
            (self.files.copies.is_empty() || ALLOWED_WHILE_COPIES_STAND.contains(&variant))
                && (self.files.kept_after_delete.is_empty() || !restart)
                && !(base_forgotten && variant == "DeleteDocument"),
            Reason::CopyStands,
        )
    }

    /// A restart keeps the copies on disk and the undone deletions beside the
    /// Loro snapshot; the pasted ancestors and a pending conflict were in
    /// memory.
    pub fn restart_forgets_copy_state(&mut self) {
        for copy in self.files.copies.values_mut() {
            copy.base_known = false;
            copy.conflict = false;
        }
        self.files.deletions_ended_by_edit.clear();
    }

    /// The document whose file holds `id`, when a file does.
    pub fn owner_doc_of(&self, id: &EntityUri) -> Option<EntityUri> {
        match self.file_home_of(id) {
            DrawnHome::File(doc) => Some(doc),
            DrawnHome::Storeless | DrawnHome::Unmodelled => None,
        }
    }

    /// The copied heading at or above `id`.
    pub fn copied_heading_above(&self, id: &EntityUri) -> Option<EntityUri> {
        let mut cursor = id.clone();
        for _ in 0..=self.domain.block_state.blocks.len() {
            if self.files.copies.contains_key(&cursor) {
                return Some(cursor);
            }
            cursor = self
                .domain
                .block_state
                .blocks
                .get(&cursor)?
                .parent_id
                .clone();
        }
        panic!("walking `{id}`'s parents outlasted the block count — the model's tree cycles");
    }

    /// `id` and every block below it, heading first.
    pub fn subtree_blocks(&self, id: &EntityUri) -> Vec<Block> {
        self.domain
            .block_state
            .subtree_ids(id)
            .iter()
            .map(|member| self.domain.block_state.blocks[member].clone())
            .collect()
    }

    /// Copies whose task keyword a copy and the store changed apart and that
    /// are not yet in conflict: releasing one now refuses the adoption.
    pub fn copies_edited_apart(&self) -> Vec<EntityUri> {
        self.copies_where(|base, disk, store| merged_keyword(base, disk, store, false).is_none())
    }

    /// Copies whose block Holon changed since the paste, while a copy did not
    /// change.
    pub fn copies_edited_in_holon(&self) -> Vec<EntityUri> {
        self.copies_where(|base, disk, store| disk == base && store != base)
    }

    fn copies_where(
        &self,
        keep: impl Fn(&Option<String>, &Option<String>, &Option<String>) -> bool,
    ) -> Vec<EntityUri> {
        self.files
            .copies
            .iter()
            .filter(|(id, copy)| {
                !copy.conflict
                    && copy.files.values().any(|on_disk| {
                        keep(
                            &keyword(&copy.pasted[0]),
                            &on_disk.disk_task_state,
                            &keyword(&self.domain.block_state.blocks[*id]),
                        )
                    })
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub fn model_copies(&self) -> Vec<ModelCopy> {
        let mut out = Vec::new();
        for (id, copy) in &self.files.copies {
            let owner = self
                .owner_doc_of(id)
                .unwrap_or_else(|| panic!("copied block {id} is in no file"));
            for doc in copy.files.keys() {
                out.push(ModelCopy {
                    block_bare: id.id().to_string(),
                    owner_file: file_name_of(self, &owner),
                    owner_doc: owner.clone(),
                    copy_file: file_name_of(self, doc),
                    copy_doc: doc.clone(),
                });
            }
        }
        out
    }

    /// The documents of `docs` in the order the app visits their files.
    pub fn in_path_order<'a>(
        &self,
        docs: impl IntoIterator<Item = &'a EntityUri>,
    ) -> Vec<EntityUri> {
        let mut docs: Vec<EntityUri> = docs.into_iter().cloned().collect();
        // Paths compare by component, as the app sorts them: `Journals/x.org`
        // comes before `Journals.org`.
        docs.sort_by_key(|doc| std::path::PathBuf::from(&self.files.documents[doc]));
        docs
    }

    /// The external editor pastes `id`'s subtree into `target_doc`'s file.
    pub fn paste_copy(&mut self, id: &EntityUri, target_doc: &EntityUri) {
        let blocks = self.subtree_blocks(id);
        let on_disk = CopyOnDisk {
            disk_task_state: keyword(&blocks[0]),
            blocks: blocks.clone(),
        };
        self.files
            .copies
            .entry(id.clone())
            .or_insert_with(|| PastedCopy {
                pasted: blocks,
                files: BTreeMap::new(),
                conflict: false,
                base_known: true,
                undone: BTreeMap::new(),
            })
            .files
            .insert(target_doc.clone(), on_disk);
    }

    /// `doc`'s file no longer holds its copy of `id`. A member whose
    /// deletion from the owner's file was undone and that no copy holds any
    /// more is deleted.
    pub fn drop_copy_file(&mut self, id: &EntityUri, doc: &EntityUri) {
        let copy = self
            .files
            .copies
            .get_mut(id)
            .unwrap_or_else(|| panic!("{doc} holds no copy of {id}"));
        copy.files.remove(doc);
        for undone in copy.undone.values_mut() {
            undone.holders.remove(doc);
        }
        self.settle_undone(id);
        if self.files.copies[id].files.is_empty() {
            self.files.copies.remove(id);
        }
    }

    /// The undone deletions of members of `id` that no copy holds any more
    /// stand: the members leave the store.
    pub fn settle_undone(&mut self, id: &EntityUri) {
        let copy = self.files.copies.get_mut(id).expect("the copy stands");
        let standing: Vec<EntityUri> = copy
            .undone
            .iter()
            .filter(|(_, undone)| undone.holders.is_empty())
            .map(|(member, _)| member.clone())
            .collect();
        for member in &standing {
            copy.undone.remove(member);
        }
        for member in standing {
            self.delete_ingested_leaf(&member);
        }
    }

    /// The owner's file let `id` go. The first copy file by path that merges
    /// with the store adopts it: every member the user deleted from the
    /// owner's file stays deleted, and the task keyword merges against the
    /// pasted one. A copy both sides changed apart is refused and the
    /// conflict stands, until the next release, which the copy wins. Other
    /// copy files hold copies of the new owner.
    pub fn release_copy(&mut self, id: &EntityUri) {
        let copy = self.files.copies[id].clone();
        let store = keyword(&self.domain.block_state.blocks[id]);
        let base = keyword(&copy.pasted[0]);
        for doc in self.in_path_order(copy.files.keys()) {
            let Some(merged) = merged_keyword(
                &base,
                &copy.files[&doc].disk_task_state,
                &store,
                copy.conflict,
            ) else {
                continue;
            };
            for member in copy.undone.keys() {
                self.delete_ingested_leaf(member);
            }
            crate::pbt::transitions::move_block_between_files::move_in_ref(
                self,
                id,
                &doc,
                &crate::pbt::block_state::Placement::First,
            );
            let block = self
                .domain
                .block_state
                .blocks
                .get_mut(id)
                .expect("adopted block is in the model");
            set_keyword(block, merged);
            let rest = self.files.copies.get_mut(id).expect("present above");
            rest.files.remove(&doc);
            rest.undone.clear();
            rest.conflict = false;
            if rest.files.is_empty() {
                self.files.copies.remove(id);
            }
            return;
        }
        self.files
            .copies
            .get_mut(id)
            .expect("present above")
            .conflict = true;
    }

    pub fn snapshot_copies(&self) -> CopiesBefore {
        CopiesBefore(
            self.files
                .copies
                .keys()
                .filter_map(|id| {
                    let owner = self.owner_doc_of(id)?;
                    Some((id.clone(), (owner, self.subtree_blocks(id))))
                })
                .collect(),
        )
    }

    /// Carry the copy map through a transition that did not speak of it:
    /// a deleted copy file drops its copy; a copied block gone with its
    /// owner's file is adopted by the first copy file by path (the copy wins
    /// conflicts: the user deleted the owner's version); a copied block
    /// deleted in Holon is brought back by its copy; a member deleted in
    /// Holon ends its undone deletion. Then state the conditions the map
    /// implies.
    pub fn follow_copies(&mut self, before: CopiesBefore) {
        self.end_edited_undone_deletions();
        for (id, (owner_before, store_subtree)) in before.0 {
            let Some(copy) = self.files.copies.get(&id).cloned() else {
                continue;
            };
            for doc in copy.files.keys() {
                if !self.files.documents.contains_key(doc) {
                    self.drop_copy_file(&id, doc);
                }
            }
            let Some(copy) = self.files.copies.get(&id).cloned() else {
                continue;
            };
            if self.domain.block_state.blocks.contains_key(&id) {
                continue;
            }
            let docs = self.in_path_order(copy.files.keys());
            let first = docs[0].clone();
            // Its deletion closed any editor on it; the block the copy brings
            // back is a new one to the editor.
            self.clear_focus_if_deleted(&id);
            if self.files.documents.contains_key(&owner_before) {
                assert_eq!(
                    docs.len(),
                    1,
                    "the model describes a Holon delete of a heading one other file holds"
                );
                self.files.copies.remove(&id);
                let on_disk = &copy.files[&first];
                let mut blocks = on_disk.blocks.clone();
                set_keyword(&mut blocks[0], on_disk.disk_task_state.clone());
                self.place_first(blocks, &first);
                self.files
                    .kept_after_delete
                    .insert(id.clone(), first.clone());
            } else {
                let mut blocks: Vec<Block> = store_subtree
                    .into_iter()
                    .filter(|b| !copy.undone.contains_key(&b.id))
                    .collect();
                let merged = merged_keyword(
                    &keyword(&copy.pasted[0]),
                    &copy.files[&first].disk_task_state,
                    &keyword(&blocks[0]),
                    true,
                )
                .expect("the copy wins every conflict");
                set_keyword(&mut blocks[0], merged);
                self.place_first(blocks, &first);
                let rest = self.files.copies.get_mut(&id).expect("present above");
                rest.files.remove(&first);
                rest.undone.clear();
                rest.conflict = false;
                if rest.files.is_empty() {
                    self.files.copies.remove(&id);
                }
            }
        }
        let blocks = &self.domain.block_state.blocks;
        for copy in self.files.copies.values_mut() {
            copy.undone.retain(|member, _| blocks.contains_key(member));
        }
        let gone: Vec<EntityUri> = self
            .files
            .kept_after_delete
            .iter()
            .filter(|(id, doc)| self.owner_doc_of(id).as_ref() != Some(*doc))
            .map(|(id, _)| id.clone())
            .collect();
        for id in gone {
            self.files.kept_after_delete.remove(&id);
        }
        self.derive_copy_conditions();
    }

    /// An undone deletion whose member Holon changed since the put-back ends:
    /// the member stays.
    fn end_edited_undone_deletions(&mut self) {
        // A member is put back childless; a child no undone deletion covers is
        // new. An ended deletion uncovers its member for its ancestors.
        while let Some((root, member)) = self.first_edited_undone_member() {
            let owner = self
                .owner_doc_of(&root)
                .unwrap_or_else(|| panic!("copied block {root} is in no file"));
            let file = file_name_of(self, &owner);
            self.files
                .copies
                .get_mut(&root)
                .expect("present above")
                .undone
                .remove(&member);
            self.files.deletions_ended_by_edit.insert(member, file);
        }
    }

    /// `(copied heading, member)` of an undone member that was edited, moved
    /// or given a child since the put-back.
    fn first_edited_undone_member(&self) -> Option<(EntityUri, EntityUri)> {
        let blocks = &self.domain.block_state.blocks;
        let covered: BTreeSet<&EntityUri> = self
            .files
            .copies
            .values()
            .flat_map(|copy| copy.undone.keys())
            .collect();
        self.files.copies.iter().find_map(|(root, copy)| {
            copy.undone.iter().find_map(|(member, undone)| {
                let now = blocks.get(member)?;
                let put_back = &undone.put_back;
                let edited = now.content != put_back.content
                    || keyword(now) != keyword(put_back)
                    || now.collapsed != put_back.collapsed
                    || now.parent_id != put_back.parent_id
                    || blocks
                        .values()
                        .any(|b| b.parent_id == *member && !covered.contains(&b.id));
                edited.then(|| (root.clone(), member.clone()))
            })
        })
    }

    /// A childless block an ingest deleted.
    pub fn delete_ingested_leaf(&mut self, id: &EntityUri) {
        self.clear_focus_if_deleted(id);
        self.domain
            .block_state
            .blocks
            .remove(id)
            .unwrap_or_else(|| panic!("the ingest deleted {id}, which the model does not hold"));
        self.domain.block_state.block_documents.remove(id);
        self.files.ingest_origin_blocks.remove(id);
        self.rebuild_profile_tracking();
    }

    /// `blocks` (a subtree, heading first) re-created by a file ingest as the
    /// first heading of `doc`'s file.
    fn place_first(&mut self, mut blocks: Vec<Block>, doc: &EntityUri) {
        let first = self
            .domain
            .block_state
            .blocks
            .values()
            .filter(|b| b.parent_id == *doc)
            .map(|b| b.sequence())
            .min()
            .unwrap_or(0);
        blocks[0].parent_id = doc.clone();
        blocks[0].set_sequence(first - 1);
        for block in blocks {
            self.domain
                .block_state
                .block_documents
                .insert(block.id.clone(), doc.clone());
            self.files.ingest_origin_blocks.insert(block.id.clone());
            self.domain
                .block_state
                .blocks
                .insert(block.id.clone(), block);
        }
        let mut all: Vec<Block> = self.domain.block_state.blocks.values().cloned().collect();
        crate::assign_reference_sequences_canonical(&mut all);
        self.domain.block_state.blocks = all.into_iter().map(|b| (b.id.clone(), b)).collect();
        self.rebuild_profile_tracking();
    }

    fn derive_copy_conditions(&mut self) {
        for kind in COPY_KINDS {
            self.conditions.clear_kind(kind);
        }
        let mut raised: Vec<(String, &'static str, Vec<String>)> = Vec::new();
        for (id, copy) in &self.files.copies {
            let owner = file_name_of(
                self,
                &self
                    .owner_doc_of(id)
                    .unwrap_or_else(|| panic!("copied block {id} is in no file")),
            );
            let names = |docs: &mut dyn Iterator<Item = &EntityUri>| -> Vec<String> {
                std::iter::once(owner.clone())
                    .chain(
                        self.in_path_order(docs)
                            .iter()
                            .map(|doc| file_name_of(self, doc)),
                    )
                    .collect()
            };
            let kind = if copy.conflict {
                holon_api::ConditionKind::BLOCK_EDITED_IN_TWO_FILES
            } else {
                holon_api::ConditionKind::BLOCK_IN_TWO_FILES
            };
            raised.push((id.as_str().to_string(), kind, names(&mut copy.files.keys())));
            for (member, undone) in &copy.undone {
                raised.push((
                    member.as_str().to_string(),
                    holon_api::ConditionKind::DELETION_UNDONE_BLOCK_IN_OTHER_FILE,
                    names(&mut undone.holders.iter()),
                ));
            }
        }
        for (id, doc) in &self.files.kept_after_delete {
            raised.push((
                id.as_str().to_string(),
                holon_api::ConditionKind::DELETED_BLOCK_KEPT_IN_FILE,
                vec![file_name_of(self, doc)],
            ));
        }
        for (id, file) in &self.files.deletions_ended_by_edit {
            raised.push((
                id.as_str().to_string(),
                holon_api::ConditionKind::DELETION_ENDED_BY_EDIT,
                vec![file.clone()],
            ));
        }
        for (subject, kind, files) in raised {
            self.conditions.raise_naming_files(subject, kind, files);
        }
    }
}

fn copies_in<'a>(
    path: &std::path::Path,
    copies: &'a holon_pbt_core::capabilities::CopiesByFile,
) -> Option<&'a std::collections::BTreeSet<String>> {
    let name = path.file_name()?.to_string_lossy();
    copies.get(name.as_ref())
}

/// The org text `disk` of the file `path` without the subtrees of the copies
/// `copies` names for it. A named copy that is not there is left to
/// `inv-copies-stay-on-disk`.
pub fn without_copy_sections(
    path: &std::path::Path,
    disk: &str,
    copies: &holon_pbt_core::capabilities::CopiesByFile,
) -> String {
    let mut text = disk.to_string();
    for bare in copies_in(path, copies).into_iter().flatten() {
        if let Some((rest, _)) = holon_orgmode::subtree::cut_subtree(&text, bare) {
            text = rest;
        }
    }
    text
}

/// `blocks`, parsed from the file `path` in document order, without the
/// subtrees of the copies `copies` names for it.
pub fn without_copy_blocks(
    path: &std::path::Path,
    blocks: Vec<Block>,
    copies: &holon_pbt_core::capabilities::CopiesByFile,
) -> Vec<Block> {
    let Some(named) = copies_in(path, copies) else {
        return blocks;
    };
    let mut skipped: std::collections::HashSet<EntityUri> = std::collections::HashSet::new();
    blocks
        .into_iter()
        .filter(|block| {
            if named.contains(block.id.id()) || skipped.contains(&block.parent_id) {
                skipped.insert(block.id.clone());
                false
            } else {
                true
            }
        })
        .collect()
}
