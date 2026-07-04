//! The ring `block.cycle_task_state` advances a block through.
//!
//! Cycling is the sibling of promotion: both write a `task_state`, so both are
//! only correct against the OWNING DOCUMENT's `#+TODO:` vocabulary. A keyword
//! outside it is unreadable to the org parser, and the next full re-ingest
//! silently demotes the headline back to body text.

use holon_org_format::TaskKeywordVocabulary;

/// The native ring an undeclaring document keeps.
const NATIVE_RING: [&str; 4] = ["", "TODO", "DOING", "DONE"];

/// The ordered ring Cmd+Enter walks: the empty (not-a-task) state, then every
/// keyword the document declares, active before done.
///
/// The DEFAULT vocabulary is deliberately NOT read as a ring. It is an INGEST
/// tolerance set — it also admits `LATER`/`NOW` (routed by `cycle_state`'s
/// LogSeq rule) and `CANCELLED`/`CLOSED` so foreign vaults parse — and walking
/// a user through those was never the behaviour. Declaring a vocabulary, by
/// contrast, makes every keyword in it a deliberate stop.
pub fn cycle_ring(vocabulary: &TaskKeywordVocabulary) -> Vec<String> {
    if *vocabulary == TaskKeywordVocabulary::default() {
        return NATIVE_RING.iter().map(|s| s.to_string()).collect();
    }
    std::iter::once(String::new())
        .chain(vocabulary.active_keywords().iter().cloned())
        .chain(vocabulary.done_keywords().iter().cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_vocabulary_yields_the_native_ring() {
        assert_eq!(
            cycle_ring(&TaskKeywordVocabulary::default()),
            vec!["", "TODO", "DOING", "DONE"]
        );
    }

    #[test]
    fn an_undeclaring_document_resolves_to_the_native_ring() {
        // `for_document` with nothing declared IS the default vocabulary.
        assert_eq!(
            cycle_ring(&TaskKeywordVocabulary::for_document(&[], &[])),
            vec!["", "TODO", "DOING", "DONE"]
        );
    }

    #[test]
    fn a_declared_vocabulary_is_the_ring_active_before_done() {
        let vocabulary = TaskKeywordVocabulary::for_document(
            &["NEXT".to_string(), "WAITING".to_string()],
            &["DONE".to_string(), "CANCELLED".to_string()],
        );
        assert_eq!(
            cycle_ring(&vocabulary),
            vec!["", "NEXT", "WAITING", "DONE", "CANCELLED"]
        );
    }
}
