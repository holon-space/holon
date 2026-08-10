//! The editable surface's source projection — the ONE place a stored authority
//! (`content` + `task_state`) becomes editor text, and the one place a
//! [`ProjectionRefusal`] is disclosed.
//!
//! The editor edits VAULT SYNTAX, not the content column: a task shows
//! `TODO milk`, and committing that raw text back through
//! [`holon_org_format::source_channel_commit`] lets the store's convergence be
//! the parse. Every authority→editor edge therefore runs through
//! [`project_or_disclose`]; an edge that seeded the stripped content instead
//! would silently mean something else.

use holon_api::EntityUri;
use holon_api::TaskState;
use holon_org_format::SourceProjection;
use holon_org_format::TaskKeywordVocabulary;

/// The `#+TODO:` vocabulary governing `block`, resolved through the query
/// capability. Read ONCE PER FOCUS, never per keystroke: it costs a
/// page-ancestor walk, and the commit path does not need it at all (the store
/// re-resolves it at the parse).
///
/// `None` means CANNOT RESOLVE — this wiring has no query capability, so there
/// is no document to ask. That is a different answer from a document that
/// declares nothing (`Some(defaults)`), and the caller must keep it different:
/// the parser's defaults declare `TODO`, so a surface classified under them
/// would be judged against a vocabulary no document ever stated. A read that
/// FAILS propagates as `Err` rather than collapsing into either.
pub async fn vocabulary_for_block(
    services: &dyn crate::reactive::BuilderServices,
    block: &EntityUri,
) -> anyhow::Result<Option<TaskKeywordVocabulary>> {
    let Some(query) = services.query_engine() else {
        // No query capability means no document to read — NOT "the document
        // declares nothing". Answering with the defaults here would be a
        // fabricated vocabulary, and the surface classified under it would be
        // wrong in exactly the direction that loses task state.
        return Ok(None);
    };
    Ok(Some(TaskKeywordVocabulary::from_declared(
        query.block_todo_keywords(block).await?,
    )))
}

/// Project a block's stored state into the text its editor shows, disclosing a
/// refusal at WARN instead of quietly seeding the stripped content.
///
/// A refused block shows its stored CONTENT, which looks identical on screen to
/// a projection and means something else — so the refusal travels back to the
/// caller as [`Surface::Refused`], which pins the commit to the content channel
/// and is what actually keeps the task state out of reach. The WARN names the
/// reason and the consequence for the human reading the log.
pub fn project_or_disclose(
    block_id: &str,
    content: &str,
    task_state: Option<&str>,
    vocabulary: &TaskKeywordVocabulary,
) -> SurfaceSeed {
    let state = task_state
        .filter(|k| !k.is_empty())
        .map(TaskState::from_keyword);
    let untasked = state.is_none();
    match holon_org_format::source_projection(state.as_ref(), content, vocabulary) {
        SourceProjection::Text(text) => SurfaceSeed {
            text,
            surface: if untasked {
                Surface::Untasked
            } else {
                Surface::Projected
            },
        },
        SourceProjection::Refused(reason) => {
            tracing::warn!(
                target: "editor.source_projection",
                block = %block_id,
                refusal = reason.as_str(),
                "cannot show this block's vault syntax ({reason}); the editor shows the stored \
                 content instead, so the task keyword is NOT editable here"
            );
            SurfaceSeed {
                text: content.to_string(),
                surface: Surface::Refused,
            }
        }
    }
}

/// What an editor seeded, and what that makes its buffer MEAN. Carried as data
/// because the commit router cannot re-derive it: whether a keyword is real is
/// the document's vocabulary to say, and the cheap shape rule the router would
/// otherwise use is vocabulary-FREE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceSeed {
    pub text: String,
    pub surface: Surface,
}

/// What an editor's buffer is, decided ONCE at the seed under the owning
/// document's vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// The owning document's vocabulary is NOT KNOWN yet (the page-ancestor
    /// read is still in flight), so the buffer has not been classified at all
    /// and shows the content column verbatim.
    ///
    /// It is a state, not a default: classifying under the parser's defaults
    /// would fabricate an answer, and the defaults declare `TODO` — so a block
    /// the real vocabulary refuses would be projected and pinned to the source
    /// channel, and an unclassified one would fall to the vocabulary-free shape
    /// rule. Both lose the task state on the next keystroke. Routing is
    /// therefore the SAFE channel until the real vocabulary arrives and the
    /// seed is re-projected.
    Pending,
    /// No task state. Any keyword the user types is adjudicated by the store's
    /// parse — which is vocabulary-aware, and which writes no task state when
    /// the text names none.
    Untasked,
    /// The buffer IS vault syntax for a declared keyword, so every commit
    /// re-derives both columns from it — including the edit that deletes the
    /// keyword, which is the demotion gesture.
    Projected,
    /// The block carries a keyword this document does not declare, so the
    /// surface shows stored content. The task is NOT editable here, and — the
    /// point — NOT REMOVABLE here either: every commit stays on the content
    /// channel, which by contract never re-derives a task state.
    ///
    /// Without this, a refused buffer whose content merely STARTS with an
    /// uppercase token (`API rewrite`, `ASAP call Bob`) passed the router's
    /// vocabulary-free shape rule, reached the source channel, and had its task
    /// silently cleared by a store that correctly found no declared keyword.
    Refused,
}

#[cfg(test)]
mod tests {
    use holon_org_format::TaskKeywordVocabulary;

    use super::Surface;
    use super::SurfaceSeed;
    use super::project_or_disclose;

    fn next_only() -> TaskKeywordVocabulary {
        TaskKeywordVocabulary::for_document(&["NEXT".to_string()], &["DONE".to_string()])
    }

    /// Inc 3 — the F3 class, reachable from imported or legacy rows: a block
    /// carrying a keyword its own document does not declare. Projecting it
    /// would put `TODO x` on screen, which THAT document's parser reads as
    /// prose — so the next commit would silently demote the task. The seed
    /// refuses and shows stored content instead.
    #[test]
    fn a_task_state_the_document_does_not_declare_is_not_projected() {
        assert_eq!(
            project_or_disclose("block:legacy", "x", Some("TODO"), &next_only()),
            SurfaceSeed {
                text: "x".into(),
                surface: Surface::Refused
            }
        );
        // And the refusal is CORRECT, not merely present: the projection it
        // declined does not parse back to this state.
        assert!(holon_org_format::converge_keyword_headed("TODO x", &next_only()).is_none());
    }

    /// The other refusal arm: content whose leading whitespace the keyword
    /// parser eats, so the projection would lose a space on the first commit.
    #[test]
    fn content_starting_with_whitespace_is_not_projected() {
        assert_eq!(
            project_or_disclose("block:legacy", " milk", Some("NEXT"), &next_only()),
            SurfaceSeed {
                text: " milk".into(),
                surface: Surface::Refused
            }
        );
    }

    /// A declared keyword projects, and a plain block is its own surface.
    #[test]
    fn a_declared_keyword_becomes_vault_syntax() {
        assert_eq!(
            project_or_disclose("block:b", "milk", Some("NEXT"), &next_only()),
            SurfaceSeed {
                text: "NEXT milk".into(),
                surface: Surface::Projected
            }
        );
        assert_eq!(
            project_or_disclose("block:b", "milk", None, &next_only()),
            SurfaceSeed {
                text: "milk".into(),
                surface: Surface::Untasked
            }
        );
        // A CLEARED task state is not a keyword — the empty string is how the
        // store spells "no task", and it must not project a bare space.
        assert_eq!(
            project_or_disclose("block:b", "milk", Some(""), &next_only()),
            SurfaceSeed {
                text: "milk".into(),
                surface: Surface::Untasked
            }
        );
    }
}
