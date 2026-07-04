//! The task-keyword vocabulary seam.
//!
//! Which words are keywords is the owning DOCUMENT's `#+TODO:` line to say, so
//! every path that parses or projects a task keyword resolves the vocabulary
//! through this trait rather than a hardcoded list.

/// Resolve the task-keyword vocabulary that governs a block: its owning
/// document's `#+TODO:` / `#+SEQ_TODO:` declaration, else the defaults.
///
/// Deliberately NOT a widening of `UndoStateReader`: that trait is undo's
/// single-row precondition reader, and a page-ancestor walk is a different
/// capability with a different failure mode.
#[async_trait::async_trait]
pub trait TaskVocabularySource: Send + Sync {
    async fn vocabulary_for_block(
        &self,
        block_id: &str,
    ) -> anyhow::Result<holon_org_format::TaskKeywordVocabulary>;
}
