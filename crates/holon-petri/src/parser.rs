//! Deterministic task-syntax boundary parser (C7; `docs/Vision/PetriNet.md`
//! §"Task Syntax: From Text to Petri Net"). This is the always-runs, zero-AI
//! stage of the progressive-enrichment pipeline: it turns a task's raw text
//! into a typed [`ParsedTask`] so that no downstream code re-scans the string
//! (parse, don't validate). AI enrichment, if any, is a later suggestion-only
//! stage layered on top — it never changes what this stage decides.
//!
//! The markers it recognizes (all optional; a bare sentence is a valid self
//! transition):
//!
//! | Marker | Meaning |
//! |---|---|
//! | leading `>` | depends on the previous sibling |
//! | `@[[Person]]:` | delegation (Person is the executor) |
//! | `@agent:` | AI/automation executor (e.g. `@perplexity:`) |
//! | leading `?` | question (produces an Information token) |
//! | `verb [[Object]]` | recognized verb operating on an object token |
//! | `#[[Tag]]` | context tag (never a dependency) |
//! | `via: [[Source]]` / `via: @agent` | question resolution route |
//! | `🔁 every X` | periodic recurrence (bridges to [`Recurrence`]) |
//!
//! ## The reference-role safe default (the correctness core)
//!
//! A `[[reference]]` plays one of three roles — executor, object, or context —
//! and getting this wrong silently invents dependencies. The safe default
//! (PetriNet.md §"Three Roles of `[[References]]`"): a bracketed reference is
//! an **object** (a real input arc) *only* when the task has a recognized verb
//! and the reference is not the `@[[…]]:` executor and not a `#[[…]]` tag.
//! Otherwise it is **context** — a tag, never an arc. This prevents a task like
//! `Überlegen, was ich [[Kat]] schenke` from being falsely blocked on Kat.

use holon_api::clock::Recurrence;

/// Who performs the transition (PetriNet.md §"The Executor Model"). The `@`
/// prefix selects which Person/agent token appears in the transition's
/// input/output arcs; its absence means the Self token (I do it myself).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Executor {
    /// No `@` prefix — the Self token is borrowed into the arcs.
    SelfExec,
    /// `@[[Person]]:` — that Person is the executor; materialization builds a
    /// delegation sub-net with a `waiting_for` token.
    Delegated { person: String },
    /// `@agent:` (e.g. `@perplexity:`, `@claude:`) — an automation executor.
    /// Async agent-firing semantics are not yet wired in the engine; until then
    /// materialization treats it like [`Executor::SelfExec`].
    Agent { name: String },
}

/// The Petri-net operation a recognized verb denotes (PetriNet.md
/// §"Verb-to-Operation Mapping"). Each maps an object token from a pre-state to
/// a post-state, or produces a knowledge token — the semantics a later
/// materialization step attaches to the object arc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerbOp {
    /// `not_exists` → `exists` (erstellen / create).
    Create,
    /// `unknown` → `researched` (recherchieren / research).
    Research,
    /// `wanted` → `ordered` (bestellen / order).
    Order,
    /// `unpaid` → `paid` (überweisen / pay).
    Pay,
    /// `draft` → `sent` (abschicken / send).
    Send,
    /// gains a `discussed` attribute (besprechen / discuss).
    Discuss,
    /// produces an Information token (checken / check).
    Check,
    /// question transition, may involve a person (fragen / ask).
    Ask,
}

impl VerbOp {
    /// The `(pre_state, post_state)` this operation moves an object token
    /// through, or `None` for the state field when the op does not
    /// gate/transition state.
    pub fn state_transition(self) -> (Option<&'static str>, Option<&'static str>) {
        match self {
            VerbOp::Create => (Some("not_exists"), Some("exists")),
            VerbOp::Research => (Some("unknown"), Some("researched")),
            VerbOp::Order => (Some("wanted"), Some("ordered")),
            VerbOp::Pay => (Some("unpaid"), Some("paid")),
            VerbOp::Send => (Some("draft"), Some("sent")),
            VerbOp::Discuss => (None, Some("discussed")),
            VerbOp::Check | VerbOp::Ask => (None, None),
        }
    }

    /// Whether firing this verb's transition yields a knowledge/information
    /// token.
    pub fn produces_knowledge(self) -> bool {
        matches!(self, VerbOp::Check | VerbOp::Ask)
    }
}

/// The default verb dictionary (~30 German + English lemmas → [`VerbOp`]).
/// Matched case-insensitively against whole words of the task text. This is the
/// built-in baseline; making it user-extensible via dictionary blocks is a
/// later step (PetriNet.md calls the dictionary "extensible per user").
///
/// Kept deliberately as lemmas plus the most common surface forms actually
/// typed; full morphological lemmatization is out of scope for the
/// deterministic stage.
pub const DEFAULT_VERB_DICT: &[(&str, VerbOp)] = &[
    // create
    ("erstellen", VerbOp::Create),
    ("erstelle", VerbOp::Create),
    ("anlegen", VerbOp::Create),
    ("schreiben", VerbOp::Create),
    ("create", VerbOp::Create),
    ("write", VerbOp::Create),
    ("make", VerbOp::Create),
    ("build", VerbOp::Create),
    // research
    ("recherchieren", VerbOp::Research),
    ("nachschauen", VerbOp::Research),
    ("research", VerbOp::Research),
    ("investigate", VerbOp::Research),
    ("find", VerbOp::Research),
    // order
    ("bestellen", VerbOp::Order),
    ("kaufen", VerbOp::Order),
    ("order", VerbOp::Order),
    ("buy", VerbOp::Order),
    // pay
    ("überweisen", VerbOp::Pay),
    ("bezahlen", VerbOp::Pay),
    ("zahlen", VerbOp::Pay),
    ("pay", VerbOp::Pay),
    // send
    ("abschicken", VerbOp::Send),
    ("senden", VerbOp::Send),
    ("schicken", VerbOp::Send),
    ("send", VerbOp::Send),
    ("submit", VerbOp::Send),
    // discuss
    ("besprechen", VerbOp::Discuss),
    ("discuss", VerbOp::Discuss),
    // check
    ("checken", VerbOp::Check),
    ("prüfen", VerbOp::Check),
    ("check", VerbOp::Check),
    ("verify", VerbOp::Check),
    // ask
    ("fragen", VerbOp::Ask),
    ("ask", VerbOp::Ask),
];

/// A recognized verb: its canonical dictionary lemma and the operation it
/// denotes. `lemma` is owned so a runtime-loaded dictionary block (not just the
/// `'static` [`DEFAULT_VERB_DICT`]) can feed [`ParsedTask::parse_with_dict`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verb {
    pub lemma: String,
    pub op: VerbOp,
}

/// A question-resolution route (PetriNet.md §"Questions and Information
/// Tokens").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViaRoute {
    /// `via: [[Source]]` — consult a document / digital-twin source.
    Source(String),
    /// `via: @agent` — route to an agent (e.g. `via: @perplexity`).
    Agent(String),
}

/// The typed result of the deterministic parse. Every field is derived once, at
/// the boundary, from the raw task text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedTask {
    /// Who executes the transition.
    pub executor: Executor,
    /// `?`-prefixed: this task produces an Information token.
    pub is_question: bool,
    /// `>`-prefixed: depends on the previous sibling.
    pub has_sequential_dep: bool,
    /// The recognized verb, if the text contains one.
    pub verb: Option<Verb>,
    /// `[[links]]` in object position — real input/output arcs. Non-empty only
    /// when [`Self::verb`] is `Some` (the safe default gate).
    pub objects: Vec<String>,
    /// `#[[tags]]` plus every non-object bracketed reference — context, never
    /// arcs.
    pub tags: Vec<String>,
    /// `via:` resolution routes (for questions).
    pub via_routes: Vec<ViaRoute>,
    /// `🔁 every X` recurrence, if present.
    pub recurrence: Option<Recurrence>,
    /// The cleaned transition label: first line with the leading markers
    /// stripped.
    pub label: String,
}

impl ParsedTask {
    /// Parse raw task text against the built-in [`DEFAULT_VERB_DICT`].
    pub fn parse(raw: &str) -> Self {
        Self::parse_with_dict(raw, DEFAULT_VERB_DICT)
    }

    /// Parse raw task text against a caller-supplied verb dictionary. The
    /// dictionary seam is what a later "dictionaries-as-blocks" step feeds
    /// a runtime-loaded dictionary into; the built-in one is
    /// [`DEFAULT_VERB_DICT`].
    pub fn parse_with_dict(raw: &str, verb_dict: &[(&str, VerbOp)]) -> Self {
        // Grammar is line-oriented: the first line is the task; any following lines
        // are opaque body kept out of the label.
        let first_line = raw.trim().lines().next().unwrap_or("").trim();
        let mut rest = first_line.to_string();

        // 1. Leading `>` (sequential dependency).
        let has_sequential_dep = rest.starts_with('>');
        if has_sequential_dep {
            rest = rest[1..].trim_start().to_string();
        }

        // 2. Leading executor `@[[Person]]:` or `@agent:`.
        let executor = if let Some(after) = rest.strip_prefix("@[[") {
            if let Some(close) = after.find("]]") {
                let person = after[..close].to_string();
                if let Some(tail) = after[close + 2..].strip_prefix(':') {
                    rest = tail.trim_start().to_string();
                    Executor::Delegated { person }
                } else {
                    Executor::SelfExec
                }
            } else {
                Executor::SelfExec
            }
        } else if rest.starts_with('@') {
            // `@agent:` — a bare (unbracketed) agent handle followed by `:`.
            match rest[1..].find(':') {
                Some(idx) if !rest[1..1 + idx].contains(char::is_whitespace) => {
                    let name = rest[1..1 + idx].to_string();
                    rest = rest[1 + idx + 1..].trim_start().to_string();
                    Executor::Agent { name }
                }
                _ => Executor::SelfExec,
            }
        } else {
            Executor::SelfExec
        };

        // 3. Leading `?` (question).
        let is_question = rest.starts_with('?');
        if is_question {
            rest = rest[1..].trim_start().to_string();
        }

        // 4. `via:` routes — extract and remove them from the label body.
        let (rest, via_routes) = extract_via_routes(&rest);

        // 5. `🔁 every X` recurrence.
        let (rest, recurrence) = extract_recurrence(&rest);

        // 6. Recognized verb (whole-word, case-insensitive dictionary lookup).
        let verb = find_verb(&rest, verb_dict);

        // 7. Reference roles. `#[[tag]]` is always context; bare `[[link]]` is an
        // object only when a verb is present, else context (the safe default).
        let (objects, tags) = classify_references(&rest, verb.is_some());

        ParsedTask {
            executor,
            is_question,
            has_sequential_dep,
            verb,
            objects,
            tags,
            via_routes,
            recurrence,
            label: rest.trim().to_string(),
        }
    }
}

/// Extract every `via: <route>` occurrence, returning the text with them
/// removed and the parsed routes in order. A route is `[[Source]]` or `@agent`
/// (up to the next whitespace).
fn extract_via_routes(text: &str) -> (String, Vec<ViaRoute>) {
    let mut routes = Vec::new();
    let mut out = String::new();
    let mut rest = text;
    while let Some(pos) = rest.find("via:") {
        out.push_str(&rest[..pos]);
        let after = rest[pos + 4..].trim_start();
        let consumed_from = rest.len() - after.len(); // absolute end of "via:" + ws
        if let Some(inner) = after.strip_prefix("[[") {
            if let Some(close) = inner.find("]]") {
                routes.push(ViaRoute::Source(inner[..close].trim().to_string()));
                rest = &after[close + 4..]; // skip "[[" .. "]]"
                let _ = consumed_from;
                continue;
            }
        }
        if let Some(agent_rest) = after.strip_prefix('@') {
            let end = agent_rest
                .find(char::is_whitespace)
                .unwrap_or(agent_rest.len());
            let name = agent_rest[..end].trim_end_matches(&[',', '.', ';'][..]);
            if !name.is_empty() {
                routes.push(ViaRoute::Agent(name.to_string()));
                rest = &agent_rest[end..];
                continue;
            }
        }
        // A `via:` with no recognizable route — keep the literal and move past it.
        out.push_str("via:");
        rest = &rest[pos + 4..];
    }
    out.push_str(rest);
    (out.trim().to_string(), routes)
}

/// Extract a `🔁 every X` recurrence, returning the text with the marker
/// removed and the parsed [`Recurrence`] if the interval parsed. A malformed
/// interval leaves the text as-is and yields `None` (the marker is not
/// load-bearing enough to fail the whole parse — the task is still a valid
/// transition).
fn extract_recurrence(text: &str) -> (String, Option<Recurrence>) {
    const REPEAT: char = '🔁';
    let Some(pos) = text.find(REPEAT) else {
        return (text.to_string(), None);
    };
    let before = &text[..pos];
    let after = &text[pos + REPEAT.len_utf8()..];
    // The recurrence spec runs to end of line; `Recurrence::parse` handles the
    // optional leading `every`.
    match Recurrence::parse(after.trim()) {
        Ok(rec) => (before.trim().to_string(), Some(rec)),
        Err(_) => (text.to_string(), None),
    }
}

/// Find the first dictionary verb appearing as a whole word (case-insensitive).
fn find_verb(text: &str, verb_dict: &[(&str, VerbOp)]) -> Option<Verb> {
    for word in text.split(|c: char| !c.is_alphabetic()) {
        if word.is_empty() {
            continue;
        }
        let lower = word.to_lowercase();
        for &(lemma, op) in verb_dict {
            if lower == lemma {
                return Some(Verb {
                    lemma: lemma.to_string(),
                    op,
                });
            }
        }
    }
    None
}

/// Split bracketed references into objects and context tags. `#[[tag]]` is
/// always a tag; a bare `[[link]]` is an object only when `has_verb`, else a
/// tag.
fn classify_references(text: &str, has_verb: bool) -> (Vec<String>, Vec<String>) {
    let mut objects = Vec::new();
    let mut tags = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(rel) = text[i..].find("[[") {
        let open = i + rel;
        let is_tag = open > 0 && bytes[open - 1] == b'#';
        let after = &text[open + 2..];
        let Some(close) = after.find("]]") else { break };
        // `[[target][display]]` — take the target before the `][`.
        let inner = &after[..close];
        let target = inner.split("][").next().unwrap_or(inner).trim().to_string();
        if !target.is_empty() {
            if is_tag || !has_verb {
                tags.push(target);
            } else {
                objects.push(target);
            }
        }
        i = open + 2 + close + 2;
    }
    (objects, tags)
}

#[cfg(test)]
mod tests {
    use holon_api::clock::Grain;

    use super::*;

    fn parse(s: &str) -> ParsedTask {
        ParsedTask::parse(s)
    }

    #[test]
    fn plain_sentence_is_a_self_transition() {
        let p = parse("Fix Garden Behave tests");
        assert_eq!(p.executor, Executor::SelfExec);
        assert!(!p.is_question && !p.has_sequential_dep);
        assert!(p.objects.is_empty() && p.tags.is_empty());
        assert_eq!(p.label, "Fix Garden Behave tests");
    }

    #[test]
    fn delegation_person_executor() {
        let p = parse("@[[Kat]]: Zeitplan erstellen");
        assert_eq!(
            p.executor,
            Executor::Delegated {
                person: "Kat".into()
            }
        );
        assert_eq!(p.label, "Zeitplan erstellen");
        // "erstellen" is a recognized verb.
        assert_eq!(p.verb.map(|v| v.op), Some(VerbOp::Create));
    }

    #[test]
    fn agent_executor_bare_handle() {
        let p = parse("@perplexity: What is capital of France");
        assert_eq!(
            p.executor,
            Executor::Agent {
                name: "perplexity".into()
            }
        );
        assert_eq!(p.label, "What is capital of France");
    }

    #[test]
    fn at_link_without_colon_is_not_an_executor() {
        // `@[[Kat]]` with no trailing `:` is not delegation.
        let p = parse("@[[Kat]] mention only");
        assert_eq!(p.executor, Executor::SelfExec);
    }

    #[test]
    fn question_with_prefix() {
        let p = parse("? Has Finanzamt charged");
        assert!(p.is_question);
        assert_eq!(p.label, "Has Finanzamt charged");
    }

    #[test]
    fn sequential_dependency_prefix() {
        let p = parse(">Fill form");
        assert!(p.has_sequential_dep);
        assert_eq!(p.label, "Fill form");
    }

    #[test]
    fn combined_prefixes_order() {
        let p = parse("> @[[Kat]]: ? clarify scope");
        assert!(p.has_sequential_dep);
        assert_eq!(
            p.executor,
            Executor::Delegated {
                person: "Kat".into()
            }
        );
        assert!(p.is_question);
        assert_eq!(p.label, "clarify scope");
    }

    #[test]
    fn verb_object_link_becomes_object_arc() {
        let p = parse("erstellen [[Rechnung DBG]]");
        assert_eq!(p.verb.map(|v| v.op), Some(VerbOp::Create));
        assert_eq!(p.objects, vec!["Rechnung DBG".to_string()]);
        assert!(p.tags.is_empty());
    }

    #[test]
    fn safe_default_bare_link_without_verb_is_context_not_object() {
        // The correctness core: no recognized verb -> the reference is context, so
        // the task is NOT falsely blocked on Kat.
        let p = parse("Überlegen was ich [[Kat]] schenke");
        assert!(p.verb.is_none(), "no dictionary verb here");
        assert!(
            p.objects.is_empty(),
            "bare link must not become an object arc"
        );
        assert_eq!(p.tags, vec!["Kat".to_string()]);
    }

    #[test]
    fn hash_tag_is_always_context_even_with_a_verb() {
        let p = parse("erstellen [[Rechnung]] #[[Finanzamt]]");
        assert_eq!(p.objects, vec!["Rechnung".to_string()]);
        assert_eq!(p.tags, vec!["Finanzamt".to_string()]);
    }

    #[test]
    fn link_target_display_form_takes_target() {
        let p = parse("erstellen [[People/Kat][Kat]]");
        assert_eq!(p.objects, vec!["People/Kat".to_string()]);
    }

    #[test]
    fn via_routes_source_and_agent() {
        let p = parse("? Has Finanzamt charged via: [[Business Bank Account]] via: @perplexity");
        assert!(p.is_question);
        assert_eq!(
            p.via_routes,
            vec![
                ViaRoute::Source("Business Bank Account".into()),
                ViaRoute::Agent("perplexity".into()),
            ]
        );
        assert!(
            !p.label.contains("via:"),
            "via routes stripped from label: {:?}",
            p.label
        );
    }

    #[test]
    fn recurrence_marker_bridges_to_recurrence() {
        let p = parse("Standup 🔁 every 2 hours");
        let rec = p.recurrence.expect("recurrence parsed");
        assert_eq!(rec.grain, Grain::Hour);
        assert_eq!(rec.count, 2);
        assert_eq!(p.label, "Standup");
    }

    #[test]
    fn malformed_recurrence_leaves_task_valid() {
        let p = parse("Birthday 🔁 every year");
        // year is coarser than day -> Recurrence rejects; the task still parses.
        assert!(p.recurrence.is_none());
        assert!(p.label.contains("Birthday"));
    }

    #[test]
    fn verb_op_state_transitions_match_spec() {
        assert_eq!(
            VerbOp::Create.state_transition(),
            (Some("not_exists"), Some("exists"))
        );
        assert_eq!(
            VerbOp::Pay.state_transition(),
            (Some("unpaid"), Some("paid"))
        );
        assert!(VerbOp::Check.produces_knowledge());
        assert!(!VerbOp::Create.produces_knowledge());
    }
}
