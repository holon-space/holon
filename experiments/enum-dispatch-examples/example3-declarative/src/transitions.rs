//! Transition enum + trait dispatch via `declarative_enum_dispatch`.
//!
//! Variant structs in submodules. Add new variant = one line here.

pub mod append_block;
pub mod create_document;
pub mod write_org_file;

use append_block::AppendBlock;
use create_document::CreateDocument;
use write_org_file::WriteOrgFile;

declarative_enum_dispatch::enum_dispatch! {
    /// Trait that every transition must implement.
    pub trait TransitionHandler {
        fn apply(&self, ctx: &mut crate::AppContext) -> anyhow::Result<()>;
        fn description(&self) -> &'static str;
    }

    // ── ADD NEW VARIANTS HERE ────────────────────────────────────
    pub enum E2ETransition {
        WriteOrgFile(WriteOrgFile),
        CreateDocument(CreateDocument),
        AppendBlock(AppendBlock),
    }
}
