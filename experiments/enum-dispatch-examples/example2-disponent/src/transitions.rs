//! Transition enum + trait — auto-dispatched by `disponent::declare!`.
//!
//! Variant structs live in submodules. To add a new transition,
//! add the variant here AND create the struct file.

pub mod append_block;
pub mod create_document;
pub mod write_org_file;

use append_block::AppendBlock;
use create_document::CreateDocument;
use write_org_file::WriteOrgFile;

disponent::declare! {
    // ── ADD NEW VARIANTS HERE ────────────────────────────────────
    pub enum E2ETransition {
        WriteOrgFile(WriteOrgFile),
        CreateDocument(CreateDocument),
        AppendBlock(AppendBlock),
    }

    pub trait TransitionHandler {
        fn apply(&self, ctx: &mut crate::AppContext) -> anyhow::Result<()>;
        fn description(&self) -> &'static str;
    }
}
