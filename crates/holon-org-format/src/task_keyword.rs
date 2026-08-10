//! Live task-keyword promotion — the one-shot dual of
//! `OrgFileFormat::reconcile_idempotent_reingest`.
//!
//! Promotion is a function of the **delta** (prior state → typed text), never
//! of the new content alone. Re-deriving it from content is the bug the
//! re-ingest reconciler exists to suppress, so this module states the rule as a
//! transition: a block that was *not* keyword-headed becomes keyword-headed.

use holon_api::TaskState;

use crate::models::DEFAULT_ACTIVE_KEYWORDS;
use crate::models::DEFAULT_DONE_KEYWORDS;

/// The closed keyword vocabulary a promotion is judged against: a document's
/// `#+TODO:` / `#+SEQ_TODO:` config when it declares one, else the defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskKeywordVocabulary {
    active: Vec<String>,
    done: Vec<String>,
}

impl Default for TaskKeywordVocabulary {
    fn default() -> Self {
        Self {
            active: DEFAULT_ACTIVE_KEYWORDS
                .iter()
                .map(|k| k.to_string())
                .collect(),
            done: DEFAULT_DONE_KEYWORDS
                .iter()
                .map(|k| k.to_string())
                .collect(),
        }
    }
}

impl TaskKeywordVocabulary {
    pub fn new(active: Vec<String>, done: Vec<String>) -> Self {
        Self { active, done }
    }

    /// A document's declared vocabulary, falling back to the defaults when it
    /// declares none — the same precedence the org parser applies.
    pub fn for_document(active: &[String], done: &[String]) -> Self {
        if active.is_empty() && done.is_empty() {
            return Self::default();
        }
        Self::new(active.to_vec(), done.to_vec())
    }

    pub fn done_keywords(&self) -> &[String] {
        &self.done
    }

    /// Every keyword this vocabulary admits, active then done — the closed set
    /// a refusal must be able to name.
    pub fn all_keywords(&self) -> Vec<String> {
        self.active
            .iter()
            .chain(self.done.iter())
            .cloned()
            .collect()
    }

    /// Every keyword, longest first, so `keyword_headed` prefers the longest
    /// match when one keyword is a prefix of another.
    fn keywords_longest_first(&self) -> Vec<&String> {
        let mut all: Vec<&String> = self.active.iter().chain(self.done.iter()).collect();
        all.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        all
    }

    fn task_state(&self, keyword: &str) -> TaskState {
        TaskState::from_keyword_with_done_list(keyword, &self.done)
    }
}

/// A detected promotion: the keyword the block becomes, and the content it
/// keeps once the keyword is stripped off the front.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Promotion {
    pub keyword: TaskState,
    pub stripped: String,
    /// Bytes the promotion removes from the FRONT of the typed text: the
    /// keyword plus exactly the whitespace it consumed. Carried rather than
    /// re-derived, so a caller shifting carets never has to assume `stripped`
    /// is still a suffix of what was typed.
    pub consumed_prefix: usize,
}

/// `s` is `KEYWORD`, then at least one ASCII whitespace, then the rest.
/// Anchored at offset 0. The returned rest still carries that whitespace.
pub fn keyword_headed<'a>(
    s: &'a str,
    vocabulary: &TaskKeywordVocabulary,
) -> Option<(TaskState, &'a str)> {
    for keyword in vocabulary.keywords_longest_first() {
        let Some(rest) = s.strip_prefix(keyword.as_str()) else {
            continue;
        };
        if !rest.starts_with(|c: char| c.is_ascii_whitespace()) {
            continue;
        }
        return Some((vocabulary.task_state(keyword), rest));
    }
    None
}

/// The leading token of `s` when it has the SHAPE of a task keyword: an
/// ASCII-uppercase-initial word of 2..=32 chars over `A-Z 0-9 - _`, then at
/// least one ASCII whitespace. Returns the token and the rest (whitespace
/// still attached).
///
/// This is deliberately vocabulary-FREE, so a caller with no document handle
/// can still tell that a keystroke MIGHT promote. It admits a strict superset
/// of [`keyword_headed`] for every vocabulary whose keywords are
/// ASCII-uppercase words — which the defaults are, and which the org
/// convention for `#+TODO:` is.
///
/// Residual it does NOT cover: a document declaring a lowercase or non-ASCII
/// keyword (`#+TODO: todo | erledigt`). Such a keystroke never reaches the
/// authority, so it does not promote.
pub fn candidate_keyword_headed(s: &str) -> Option<(&str, &str)> {
    let end = s
        .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-' || c == '_'))
        .unwrap_or(s.len());
    let (token, rest) = s.split_at(end);
    if !(2..=32).contains(&token.len()) || !token.starts_with(|c: char| c.is_ascii_uppercase()) {
        return None;
    }
    rest.starts_with(|c: char| c.is_ascii_whitespace())
        .then_some((token, rest))
}

/// [`detect_keyword_promotion`] with the vocabulary membership test replaced by
/// [`candidate_keyword_headed`]'s shape test — the proposal a caller that
/// cannot see the owning document's `#+TODO:` line makes, for the authority to
/// adjudicate against the real vocabulary.
///
/// `keyword` carries the ACTIVE category unconditionally: only the document's
/// vocabulary says which keywords are done ones, so the category here is a
/// placeholder the authority replaces.
pub fn candidate_promotion(
    prior_content: &str,
    prior_task_state: Option<&TaskState>,
    typed: &str,
) -> Option<Promotion> {
    if prior_task_state.is_some() {
        return None;
    }
    let (token, rest) = candidate_keyword_headed(typed)?;
    if candidate_keyword_headed(prior_content).is_some() {
        return None;
    }
    let stripped = rest.trim_start();
    Some(Promotion {
        consumed_prefix: token.len() + (rest.len() - stripped.len()),
        keyword: TaskState::active(token),
        stripped: stripped.to_string(),
    })
}

/// The one-shot promotion rule. Returns `Some` iff all three guards hold:
/// the block carries no task state yet, the typed text is keyword-headed, and
/// the prior content was *not* — the last is what makes promotion a transition
/// rather than a property of the text, so re-committing an unchanged
/// keyword-headed line never re-promotes.
pub fn detect_keyword_promotion(
    prior_content: &str,
    prior_task_state: Option<&TaskState>,
    typed: &str,
    vocabulary: &TaskKeywordVocabulary,
) -> Option<Promotion> {
    if prior_task_state.is_some() {
        return None;
    }
    let (keyword, rest) = keyword_headed(typed, vocabulary)?;
    if keyword_headed(prior_content, vocabulary).is_some() {
        return None;
    }
    let stripped = rest.trim_start();
    Some(Promotion {
        consumed_prefix: keyword.keyword.len() + (rest.len() - stripped.len()),
        keyword,
        stripped: stripped.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab() -> TaskKeywordVocabulary {
        TaskKeywordVocabulary::default()
    }

    fn promote(prior: &str, ts: Option<TaskState>, typed: &str) -> Option<Promotion> {
        detect_keyword_promotion(prior, ts.as_ref(), typed, &vocab())
    }

    /// G1 — the primary path: `T`→`TO`→`TOD`→`TODO` commits with prior
    /// `"TODO"`, which is EQUAL to the keyword; only a transition-shaped guard
    /// admits the space keystroke that follows.
    #[test]
    fn char_by_char_promotes_on_the_space() {
        let p = promote("TODO", None, "TODO ").expect("the space must promote");
        assert_eq!(p.keyword, TaskState::active("TODO"));
        assert_eq!(p.stripped, "");
    }

    /// G2 — guard 1: an already-promoted block never re-promotes.
    #[test]
    fn keystroke_after_promotion_does_not_refire() {
        assert_eq!(promote("", Some(TaskState::active("TODO")), "b"), None);
    }

    /// G3 — paste into an empty block: guard 3 holds trivially.
    #[test]
    fn paste_into_empty_promotes() {
        let p = promote("", None, "TODO buy milk").expect("paste must promote");
        assert_eq!(p.keyword, TaskState::active("TODO"));
        assert_eq!(p.stripped, "buy milk");
    }

    /// G4 — prepending the keyword to existing text is the same authoring
    /// gesture and promotes.
    #[test]
    fn prepend_to_existing_promotes() {
        let p = promote("buy milk", None, "TODO buy milk").expect("prepend must promote");
        assert_eq!(p.keyword, TaskState::active("TODO"));
        assert_eq!(p.stripped, "buy milk");
    }

    /// G5 — re-committing unchanged keyword-headed text does not re-fire; the
    /// guard holds independently of any caller-side unchanged-text shortcut,
    /// because it is also reachable from replay.
    #[test]
    fn recommit_of_unchanged_text_does_not_refire() {
        assert_eq!(promote("TODO buy milk", None, "TODO buy milk"), None);
    }

    /// G6 — the re-ingest-suppression block: a plain block that merely STARTS
    /// with a keyword has no task state, so guard 1 does not protect it. Guard
    /// 3 does. Losing this re-opens the double-promotion bug.
    #[test]
    fn plain_block_that_merely_starts_with_a_keyword_never_promotes() {
        assert_eq!(promote("TODO list ideas", None, "TODO list ideasX"), None);
    }

    /// G7 — the keyword must be a whole token.
    #[test]
    fn keyword_without_whitespace_does_not_promote() {
        assert_eq!(promote("", None, "TODOx"), None);
    }

    /// G8 — a bare keyword defers to the space, so promotion happens at one
    /// predictable keystroke (G1) instead of twice.
    #[test]
    fn bare_keyword_defers_to_the_space() {
        assert_eq!(promote("", None, "TODO"), None);
    }

    /// G9 — the done category comes from the vocabulary's done list.
    #[test]
    fn done_keyword_promotes_with_done_category() {
        let p = promote("DONE", None, "DONE ").expect("DONE must promote");
        assert_eq!(p.keyword, TaskState::done("DONE"));
        assert!(p.keyword.is_done());
        assert_eq!(p.stripped, "");
    }

    /// G10 — the vocabulary is closed.
    #[test]
    fn unknown_keyword_does_not_promote() {
        assert_eq!(promote("", None, "MAYBE x"), None);
    }

    /// G11 — select-all-replace reaches the commit with an unrelated prior, so
    /// guard 3 holds and it promotes. Documented, accepted behaviour.
    #[test]
    fn select_all_replace_promotes() {
        let p = promote("x", None, "TODO ").expect("replace must promote");
        assert_eq!(p.keyword, TaskState::active("TODO"));
        assert_eq!(p.stripped, "");
    }

    /// A document's own `#+TODO:` vocabulary decides both membership and
    /// category — `TODO` is not a keyword in a document that never declares it.
    #[test]
    fn document_vocabulary_is_authoritative() {
        let v =
            TaskKeywordVocabulary::for_document(&["NEXT".to_string()], &["SHIPPED".to_string()]);
        assert_eq!(detect_keyword_promotion("", None, "TODO x", &v), None);
        let p = detect_keyword_promotion("", None, "NEXT x", &v).expect("NEXT must promote");
        assert_eq!(p.keyword, TaskState::active("NEXT"));
        let d = detect_keyword_promotion("", None, "SHIPPED x", &v).expect("SHIPPED must promote");
        assert!(d.keyword.is_done());
    }

    /// The vocabulary-free gate must never miss what a real vocabulary would
    /// promote: it admits every default keyword AND a declared one it has
    /// never heard of.
    #[test]
    fn candidate_shape_is_a_superset_of_every_uppercase_vocabulary() {
        for kw in vocab().all_keywords() {
            let typed = format!("{kw} x");
            assert!(
                candidate_promotion("", None, &typed).is_some(),
                "the gate must admit the default keyword {kw}"
            );
        }
        let declared = candidate_promotion("", None, "NEXT call bank")
            .expect("an undeclared-to-the-gate keyword must still be admitted");
        assert_eq!(declared.keyword.keyword, "NEXT");
        assert_eq!(declared.stripped, "call bank");
    }

    /// The shape rule stays narrow enough that ordinary prose does not
    /// propose: lowercase words, one-letter words, and mixed case are out.
    #[test]
    fn candidate_shape_refuses_ordinary_prose() {
        for typed in ["buy milk", "I think", "Todo x", "A b", "TODOx y"] {
            assert_eq!(
                candidate_promotion("", None, typed),
                None,
                "{typed:?} must not propose a promotion"
            );
        }
    }

    /// The two guards that make it a TRANSITION carry over unchanged.
    #[test]
    fn candidate_shape_keeps_the_transition_guards() {
        assert_eq!(
            candidate_promotion("", Some(&TaskState::active("NEXT")), "NEXT x"),
            None,
            "an already-tasked block never re-proposes"
        );
        assert_eq!(
            candidate_promotion("NEXT list", None, "NEXT listX"),
            None,
            "prior text that was already candidate-headed never proposes"
        );
    }

    /// Tabs count as the separating whitespace, and multiple spaces collapse
    /// out of the stripped remainder.
    #[test]
    fn any_ascii_whitespace_separates_and_is_stripped() {
        let p = promote("", None, "TODO\t  buy milk").expect("tab must separate");
        assert_eq!(p.stripped, "buy milk");
    }
}
