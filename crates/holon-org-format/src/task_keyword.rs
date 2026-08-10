//! The task-keyword convergence rule.
//!
//! A block that carries no task state and whose content begins with a keyword
//! of its own document's vocabulary is an ILLEGAL STATE: the org file those
//! bytes render to reads back as a task, so the store would be holding a
//! reading the file disagrees with. The rule is therefore a property of the
//! CONTENT plus the document's vocabulary — not of a delta — and every write
//! path converges it at the store boundary.
//!
//! The vocabulary is the plug-in point: a format provider that declares no
//! keywords converges nothing, so `- TODO ...` stays cleartext there.
//!
//! The editable surface is the INVERSE of that rule: [`source_projection`]
//! renders the vault syntax a block's stored state spells, and an editor
//! commits that raw text back through the source channel
//! ([`source_channel_commit`]), where the same convergence re-derives both
//! fields. Editor and store therefore share ONE rule and there is no
//! delta-shaped proposal to refuse.

use std::fmt;

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

    pub fn active_keywords(&self) -> &[String] {
        &self.active
    }

    pub fn done_keywords(&self) -> &[String] {
        &self.done
    }

    /// The vocabulary a document's `#+TODO:` declaration spells, with the
    /// parser's defaults applied when it declares none (`None`). The ONE place
    /// a declaration becomes a closed vocabulary, so the store's parse and the
    /// editor's projection cannot drift apart.
    pub fn from_declared(declared: Option<Vec<TaskState>>) -> Self {
        let Some(states) = declared else {
            return Self::default();
        };
        let active: Vec<String> = states
            .iter()
            .filter(|s| s.is_active())
            .map(|s| s.keyword.clone())
            .collect();
        let done: Vec<String> = states
            .iter()
            .filter(|s| s.is_done())
            .map(|s| s.keyword.clone())
            .collect();
        Self::for_document(&active, &done)
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

/// `s` is `KEYWORD`, then either the end of the string or at least one ASCII
/// whitespace and the rest. Anchored at offset 0. The returned rest still
/// carries that whitespace.
///
/// The end-of-string arm is not an edge case: `TODO` alone renders to the
/// headline `** TODO`, which org reads as a task with an empty title.
pub fn keyword_headed<'a>(
    s: &'a str,
    vocabulary: &TaskKeywordVocabulary,
) -> Option<(TaskState, &'a str)> {
    for keyword in vocabulary.keywords_longest_first() {
        let Some(rest) = s.strip_prefix(keyword.as_str()) else {
            continue;
        };
        if !(rest.is_empty() || rest.starts_with(|c: char| c.is_ascii_whitespace())) {
            continue;
        }
        return Some((vocabulary.task_state(keyword), rest));
    }
    None
}

/// The vocabulary-free superset of [`keyword_headed`] — end-of-string included.
///
/// This is the STORE's cheap pre-filter: a content write whose shape cannot
/// converge under ANY ASCII-uppercase vocabulary is answered without reading
/// the owning document at all, which is what keeps ordinary prose off the
/// vocabulary lookup. [`source_channel_commit`] reuses it for the editor's
/// commit routing, so both sides admit exactly the same shapes.
pub fn could_converge(s: &str) -> bool {
    let end = s
        .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-' || c == '_'))
        .unwrap_or(s.len());
    let (token, rest) = s.split_at(end);
    (2..=32).contains(&token.len())
        && token.starts_with(|c: char| c.is_ascii_uppercase())
        && (rest.is_empty() || rest.starts_with(|c: char| c.is_ascii_whitespace()))
}

/// Which channel an EDITOR buffer commit takes: the source channel (the store
/// re-derives `content` + `task_state` from the raw text) or the ordinary
/// content channel (the store writes the bytes and never touches the task
/// state).
///
/// Both arms are needed and each is a distinct gesture. `new_text` catches
/// authoring a keyword; `prior_buffer` catches DELETING one — a buffer that was
/// keyword-headed and no longer is must demote the block, which only the source
/// channel can do. Everything else stays on the content channel, so ordinary
/// prose costs no document read, exactly as [`could_converge`] keeps it off the
/// store's lookup.
pub fn source_channel_commit(prior_buffer: &str, new_text: &str) -> bool {
    could_converge(new_text) || could_converge(prior_buffer)
}

/// What the editable surface shows for a block's task-keyword facet, or why it
/// cannot show it. A refusal travels as DATA, so seeding the stripped content
/// instead — which looks identical and means something else — cannot happen
/// without the caller reading the reason and disclosing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceProjection {
    /// The vault syntax for this block: what the editor seeds, and what a
    /// commit of an unedited buffer parses straight back into.
    Text(String),
    /// Projecting would produce text that does NOT parse back to this state, so
    /// the caller must seed the stored content instead and say so. Both arms
    /// are reachable from imported or legacy rows, never from a converged
    /// write.
    Refused(ProjectionRefusal),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionRefusal {
    /// The block carries a keyword this document's vocabulary does not declare.
    /// Projecting `TODO x` into a `#+TODO: NEXT | DONE` document would parse
    /// back as prose — the commit would SILENTLY DEMOTE the task.
    KeywordNotDeclared {
        keyword: String,
        vocabulary: Vec<String>,
    },
    /// The content starts with whitespace, which the parser eats: `TODO  milk`
    /// parses back to `milk`, losing the leading space on the first commit.
    ContentStartsWithWhitespace { content: String },
}

impl ProjectionRefusal {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::KeywordNotDeclared { .. } => "keyword_not_declared",
            Self::ContentStartsWithWhitespace { .. } => "content_starts_with_whitespace",
        }
    }
}

impl fmt::Display for ProjectionRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::KeywordNotDeclared {
                keyword,
                vocabulary,
            } => write!(
                f,
                "task keyword {keyword:?} is not declared by this document {vocabulary:?}, so \
                 projecting it would read back as ordinary text and demote the task"
            ),
            Self::ContentStartsWithWhitespace { content } => write!(
                f,
                "content {content:?} starts with whitespace, which the keyword parser eats — the \
                 projection would not round-trip"
            ),
        }
    }
}

/// The editable surface's view of a block: vault syntax for the task-keyword
/// facet, stored content otherwise. The INVERSE of [`converge_keyword_headed`],
/// and the pair is a fixed point — which is what lets the editor commit its
/// buffer as ordinary content and let the store's convergence be the parse.
///
/// Empty content projects to the bare keyword with NO trailing space, because
/// the store canonicalizer trims one and the projection has to survive that.
pub fn source_projection(
    task_state: Option<&TaskState>,
    content: &str,
    vocabulary: &TaskKeywordVocabulary,
) -> SourceProjection {
    let Some(task_state) = task_state else {
        return SourceProjection::Text(content.to_string());
    };
    if !vocabulary
        .all_keywords()
        .iter()
        .any(|k| *k == task_state.keyword)
    {
        return SourceProjection::Refused(ProjectionRefusal::KeywordNotDeclared {
            keyword: task_state.keyword.clone(),
            vocabulary: vocabulary.all_keywords(),
        });
    }
    if content.starts_with(|c: char| c.is_ascii_whitespace()) {
        return SourceProjection::Refused(ProjectionRefusal::ContentStartsWithWhitespace {
            content: content.to_string(),
        });
    }
    SourceProjection::Text(if content.is_empty() {
        task_state.keyword.clone()
    } else {
        format!("{} {}", task_state.keyword, content)
    })
}

/// The convergence rule. `Some` iff `content` is keyword-headed in
/// `vocabulary` — in which case the block IS the returned task, whatever the
/// write that produced the content thought it was doing.
///
/// State-based, not delta-based: the same bytes must converge whether they
/// arrive by typing, by an agent's `set_field`, by an undo replay or by a
/// split. A caller applies it only to a block that carries no task state yet;
/// a block that already has one is representable as it stands.
pub fn converge_keyword_headed(
    content: &str,
    vocabulary: &TaskKeywordVocabulary,
) -> Option<Promotion> {
    let (keyword, rest) = keyword_headed(content, vocabulary)?;
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

    fn converge(content: &str) -> Option<Promotion> {
        converge_keyword_headed(content, &vocab())
    }

    /// G1 — the primary authoring path: the keyword plus a space IS a task.
    #[test]
    fn keyword_and_space_converges() {
        let p = converge("TODO ").expect("the space must converge");
        assert_eq!(p.keyword, TaskState::active("TODO"));
        assert_eq!(p.stripped, "");
    }

    /// G3 — pasted keyword-headed text converges to keyword + remainder.
    #[test]
    fn keyword_headed_text_converges() {
        let p = converge("TODO buy milk").expect("keyword-headed text must converge");
        assert_eq!(p.keyword, TaskState::active("TODO"));
        assert_eq!(p.stripped, "buy milk");
    }

    /// G6/G8 — the SPECIFIED empty-remainder case (ruling 2026-08-10): a bare
    /// keyword renders to the headline `** TODO`, which org reads back as a
    /// task with an empty title, so the store holds exactly that. It replaces
    /// the retired "a bare keyword defers to the space" rule, which left the
    /// store holding a reading the file disagreed with.
    #[test]
    fn bare_keyword_converges_to_an_empty_titled_task() {
        let p = converge("TODO").expect("a bare keyword must converge");
        assert_eq!(p.keyword, TaskState::active("TODO"));
        assert_eq!(p.stripped, "");
        assert_eq!(p.consumed_prefix, 4);
    }

    /// G7 — the keyword must be a whole token.
    #[test]
    fn keyword_without_a_boundary_does_not_converge() {
        assert_eq!(converge("TODOx"), None);
        assert_eq!(converge("TODOx y"), None);
    }

    /// G9 — the done category comes from the vocabulary's done list.
    #[test]
    fn done_keyword_converges_with_done_category() {
        let p = converge("DONE ").expect("DONE must converge");
        assert_eq!(p.keyword, TaskState::done("DONE"));
        assert!(p.keyword.is_done());
    }

    /// G10 — the vocabulary is closed.
    #[test]
    fn unknown_keyword_does_not_converge() {
        assert_eq!(converge("MAYBE x"), None);
    }

    /// A document's own `#+TODO:` vocabulary decides both membership and
    /// category, and it is the whole plug-in surface: `TODO` is ordinary text
    /// in a document that never declares it, and a provider that declares no
    /// keywords at all converges nothing.
    #[test]
    fn document_vocabulary_is_authoritative() {
        let v =
            TaskKeywordVocabulary::for_document(&["NEXT".to_string()], &["SHIPPED".to_string()]);
        assert_eq!(converge_keyword_headed("TODO x", &v), None);
        let p = converge_keyword_headed("NEXT call bank", &v).expect("NEXT must converge");
        assert_eq!(p.keyword, TaskState::active("NEXT"));
        assert_eq!(p.stripped, "call bank");
        let d = converge_keyword_headed("SHIPPED it", &v).expect("SHIPPED must converge");
        assert!(d.keyword.is_done());

        let empty = TaskKeywordVocabulary::new(Vec::new(), Vec::new());
        assert_eq!(converge_keyword_headed("TODO x", &empty), None);
        assert_eq!(converge_keyword_headed("NEXT x", &empty), None);
    }

    /// The editor's commit routing, both directions of the one gesture:
    /// authoring a keyword AND deleting one take the source channel, because
    /// only that channel can set — or clear — the task state. Prose never does.
    #[test]
    fn source_channel_routes_both_directions_of_the_keyword_gesture() {
        assert!(
            source_channel_commit("TOD", "TODO"),
            "bare keyword authored"
        );
        assert!(source_channel_commit("TODO", "TODO milk"), "keyword headed");
        assert!(
            source_channel_commit("TODO milk", "milk"),
            "deleting the keyword must reach the channel that can demote"
        );
        assert!(source_channel_commit("TODO milk", ""), "buffer cleared");
        assert!(!source_channel_commit("buy", "buy milk"), "ordinary prose");
        assert!(!source_channel_commit("", "x"));
    }

    /// Tabs count as the separating whitespace, and multiple spaces collapse
    /// out of the stripped remainder.
    #[test]
    fn any_ascii_whitespace_separates_and_is_stripped() {
        let p = converge("TODO\t  buy milk").expect("tab must separate");
        assert_eq!(p.stripped, "buy milk");
        assert_eq!(p.consumed_prefix, 7);
    }
}
