//! The Gherkin step vocabulary: one template per transition, rendered to a
//! step and parsed back to exactly one transition value.
//!
//! Two traits carry the whole contract:
//!
//! - [`StepField`] decides **by type** how a field appears in a step.
//!   String-ish fields render quoted (with `\"` / `\\` escapes) and parse by
//!   consuming one balanced quoted run; numbers and enums render bare. A
//!   template author never chooses quoting, so a `content` field holding the
//!   literal text `" in region "` cannot confuse the parser.
//! - [`StepVocabulary`] is derived (`#[derive(StepVocabulary)]` +
//!   `#[step_template("…")]`), never hand-listed: placeholder names are checked
//!   against the real struct fields at compile time.
//!
//! Parsing is anchored — literal segments must match exactly — so the only
//! residual ambiguity is at the skeleton level, which
//! [`check_template_ambiguity`] refuses at registration time.

use proptest::strategy::BoxedStrategy;
use proptest::strategy::Strategy;

/// A step as it appears in a `.feature` file: the one-line text plus, for the
/// transitions whose payload is a document, its docstring.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedStep {
    pub text: String,
    pub docstring: Option<String>,
}

impl RenderedStep {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            docstring: None,
        }
    }
}

/// One placeholder of a template: its field name and whether its type renders
/// quoted.
pub type TemplateField = (&'static str, bool);

/// How one field of a transition appears inside a step.
///
/// `QUOTED` is a property of the TYPE. Implement it once per type; template
/// authors never repeat the decision.
pub trait StepField: Sized {
    const QUOTED: bool;

    /// The field's payload, WITHOUT the surrounding quotes (the caller adds and
    /// escapes them when `QUOTED`).
    fn render_step_field(&self) -> String;

    /// Inverse of [`render_step_field`](Self::render_step_field), over the
    /// already-unquoted payload.
    fn parse_step_field(raw: &str) -> Result<Self, String>;

    /// Values the catalog-wide round-trip property draws from. Must be
    /// non-empty; every entry must survive `parse(render(v)) == v`.
    fn step_field_examples() -> Vec<Self>;
}

/// A transition that can be written as, and read back from, a Gherkin step.
///
/// Derive it — the derive is the single authoring form:
///
/// ```ignore
/// #[derive(Clone, Debug, Serialize, Deserialize, StepVocabulary)]
/// #[step_template("I click block {block_id} in region {region}")]
/// pub struct ClickBlock {
///     pub block_id: EntityUri,
///     pub region: Region,
/// }
/// ```
pub trait StepVocabulary: Sized {
    const TEMPLATE: &'static str;

    /// EVERY field of the struct, in declaration order. Derived from the
    /// struct itself, so it cannot drift from the type.
    fn field_names() -> &'static [&'static str];

    /// The template's placeholders in template order, with their types'
    /// quoting.
    fn template_fields() -> &'static [TemplateField];

    fn render_step(&self) -> RenderedStep;

    /// `Ok(None)` = this template does not describe `text` (try another
    /// variant). `Err` = the template matched but a field would not parse —
    /// a hard error, never a silent skip.
    fn parse_step(text: &str, docstring: Option<&str>) -> Result<Option<Self>, String>;

    /// Catalog values for the round-trip property, built from each field's
    /// [`StepField::step_field_examples`].
    fn step_examples() -> Vec<Self>;

    /// The value as serde sees it — the equality used by the round-trip law
    /// (transitions are not `PartialEq`).
    fn step_json(&self) -> serde_json::Value;
}

/// One segment of a parsed template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    Literal(String),
    Field(String),
}

/// Split a template into literal / placeholder segments.
///
/// `{{` and `}}` are not supported: no template needs a literal brace, and
/// accepting them would make the escape rules two-sided for no reader benefit.
pub fn parse_template(template: &str) -> Result<Vec<Segment>, String> {
    let mut out = Vec::new();
    let mut literal = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        literal.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let close = after
            .find('}')
            .ok_or_else(|| format!("unterminated `{{` in template {template:?}"))?;
        let name = &after[..close];
        if name.is_empty() {
            return Err(format!("empty placeholder `{{}}` in template {template:?}"));
        }
        if name.contains('{') {
            return Err(format!("nested `{{` in template {template:?}"));
        }
        out.push(Segment::Literal(std::mem::take(&mut literal)));
        out.push(Segment::Field(name.to_string()));
        rest = &after[close + 1..];
    }
    literal.push_str(rest);
    out.push(Segment::Literal(literal));
    Ok(out)
}

/// The template's placeholder names, in order.
pub fn template_placeholders(template: &str) -> Result<Vec<String>, String> {
    Ok(parse_template(template)?
        .into_iter()
        .filter_map(|s| match s {
            Segment::Field(n) => Some(n),
            Segment::Literal(_) => None,
        })
        .collect())
}

/// The template's literal segments, in order — the skeleton the ambiguity
/// check compares.
pub fn template_literals(template: &str) -> Result<Vec<String>, String> {
    Ok(parse_template(template)?
        .into_iter()
        .filter_map(|s| match s {
            Segment::Literal(l) => Some(l),
            Segment::Field(_) => None,
        })
        .collect())
}

fn quote(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 2);
    out.push('"');
    for c in raw.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Consume one balanced quoted run at the start of `text`, returning the
/// unescaped payload and the remainder.
fn take_quoted(text: &str) -> Option<(String, &str)> {
    let mut chars = text.char_indices();
    if chars.next()?.1 != '"' {
        return None;
    }
    let mut payload = String::new();
    let mut escaped = false;
    for (idx, c) in chars {
        if escaped {
            payload.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '"' => return Some((payload, &text[idx + 1..])),
            _ => payload.push(c),
        }
    }
    None
}

/// Render `template`, substituting each placeholder with its value (quoted
/// when the field's type says so).
///
/// `values` must cover every placeholder; a missing one is a derive bug and
/// panics rather than rendering a half-step.
pub fn render_template(template: &str, values: &[(&str, bool, String)]) -> String {
    let segments = parse_template(template)
        .unwrap_or_else(|e| panic!("template {template:?} is not renderable: {e}"));
    let mut out = String::new();
    for segment in segments {
        match segment {
            Segment::Literal(l) => out.push_str(&l),
            Segment::Field(name) => {
                let (_, quoted, raw) = values
                    .iter()
                    .find(|(n, _, _)| *n == name)
                    .unwrap_or_else(|| panic!("template {template:?} has no value for {name:?}"));
                if *quoted {
                    out.push_str(&quote(raw));
                } else {
                    out.push_str(raw);
                }
            }
        }
    }
    out
}

/// Anchored match of `text` against `template`. `None` = this template does
/// not describe the text.
pub fn capture_template(
    template: &str,
    fields: &[TemplateField],
    text: &str,
) -> Option<Vec<(String, String)>> {
    let segments = parse_template(template)
        .unwrap_or_else(|e| panic!("template {template:?} is not parseable: {e}"));
    let mut rest = text;
    let mut captures = Vec::new();
    let mut idx = 0usize;
    while idx < segments.len() {
        match &segments[idx] {
            Segment::Literal(l) => {
                rest = rest.strip_prefix(l.as_str())?;
                idx += 1;
            }
            Segment::Field(name) => {
                let quoted = fields.iter().find(|(n, _)| n == name).map(|(_, q)| *q)?;
                if quoted {
                    let (payload, remainder) = take_quoted(rest)?;
                    captures.push((name.clone(), payload));
                    rest = remainder;
                } else {
                    // A bare field runs up to the next literal segment (or to
                    // the end of the step when it is the last one).
                    let next_literal = match segments.get(idx + 1) {
                        Some(Segment::Literal(l)) if !l.is_empty() => Some(l.as_str()),
                        _ => None,
                    };
                    let end = match next_literal {
                        Some(l) => rest.find(l)?,
                        None => rest.len(),
                    };
                    captures.push((name.clone(), rest[..end].to_string()));
                    rest = &rest[end..];
                }
                idx += 1;
            }
        }
    }
    if rest.is_empty() {
        Some(captures)
    } else {
        None
    }
}

/// Look one captured field up. Absence is a derive bug, not user input.
pub fn captured<'a>(captures: &'a [(String, String)], name: &str) -> &'a str {
    captures
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
        .unwrap_or_else(|| panic!("template capture is missing field {name:?}"))
}

/// Registration-time structural refusal (guarantee (a), layer 1).
///
/// Two templates are structurally ambiguous when their literal skeletons are
/// equal, or when one's leading literal is a prefix of another's and both take
/// the same number of fields — in both cases a rendered step could be read by
/// either.
pub fn check_template_ambiguity(entries: &[(&'static str, &'static str)]) -> Result<(), String> {
    let mut parsed: Vec<(&str, Vec<String>, usize)> = Vec::new();
    for (name, template) in entries {
        let literals = template_literals(template)
            .map_err(|e| format!("{name}: template {template:?} is malformed: {e}"))?;
        let arity = template_placeholders(template).unwrap().len();
        parsed.push((name, literals, arity));
    }
    for i in 0..parsed.len() {
        for j in (i + 1)..parsed.len() {
            let (a_name, a_lits, a_arity) = &parsed[i];
            let (b_name, b_lits, b_arity) = &parsed[j];
            if a_lits == b_lits {
                return Err(format!(
                    "step templates of {a_name} and {b_name} have identical literal skeletons \
                     {a_lits:?} — a rendered step would be readable as either"
                ));
            }
            if a_arity != b_arity {
                continue;
            }
            // At equal arity two skeletons can only overlap when EVERY literal
            // segment of one is prefix-compatible with the other's: a position
            // where neither prefixes the other (` up` vs ` down`) makes the two
            // mutually unreachable.
            let all_prefix_compatible = a_lits
                .iter()
                .zip(b_lits.iter())
                .all(|(a, b)| a.starts_with(b.as_str()) || b.starts_with(a.as_str()));
            if all_prefix_compatible {
                return Err(format!(
                    "step templates of {a_name} ({a_lits:?}) and {b_name} ({b_lits:?}) are \
                     prefix-compatible at every literal segment, at equal arity {a_arity} — a \
                     rendered step could be read as either; disambiguate one of them"
                ));
            }
        }
    }
    Ok(())
}

/// Registration-time coverage check (guarantees (c) and (e), runtime half).
///
/// The derive already refuses, at compile time, a placeholder that names no
/// field and a field that is neither templated nor defaulted. This catches
/// what the derive cannot see: a `#[serde(rename)]`/`skip` that makes the
/// serialized key set differ from the declared fields, which would silently
/// break capture replay.
pub fn check_field_coverage(
    variant: &str,
    template: &str,
    declared_fields: &[&str],
    example: &serde_json::Value,
) -> Result<(), String> {
    let placeholders = template_placeholders(template)
        .map_err(|e| format!("{variant}: template {template:?} is malformed: {e}"))?;
    for p in &placeholders {
        if !declared_fields.contains(&p.as_str()) {
            return Err(format!(
                "{variant}: template placeholder {{{p}}} names no field of the struct \
                 (fields: {declared_fields:?})"
            ));
        }
    }
    let serialized: Vec<String> = match example {
        serde_json::Value::Object(map) => map.keys().cloned().collect(),
        // Unit structs serialize to `null`; they have no fields to cover.
        serde_json::Value::Null => Vec::new(),
        other => {
            return Err(format!(
                "{variant}: a step transition must serialize to a JSON object or null, got {other}"
            ));
        }
    };
    let mut declared: Vec<String> = declared_fields.iter().map(|s| s.to_string()).collect();
    let mut serialized_sorted = serialized.clone();
    declared.sort();
    serialized_sorted.sort();
    if declared != serialized_sorted {
        return Err(format!(
            "{variant}: serde key set {serialized_sorted:?} differs from the struct's declared \
             fields {declared:?} — a rename/skip would make recorded steps unreplayable"
        ));
    }
    Ok(())
}

/// The part of a step value the round-trip law compares.
///
/// A block's birth stamps are wall-clock and no step form carries them (an org
/// docstring least of all), so re-reading a rendered step mints fresh ones.
/// Everything else must be identical.
pub fn comparable_step_value(mut value: serde_json::Value) -> serde_json::Value {
    match &mut value {
        serde_json::Value::Object(map) => {
            map.remove("created_at");
            map.remove("updated_at");
            for (_, v) in map.iter_mut() {
                *v = comparable_step_value(v.take());
            }
        }
        serde_json::Value::Array(items) => {
            for v in items.iter_mut() {
                *v = comparable_step_value(v.take());
            }
        }
        _ => {}
    }
    value
}

/// Build a strategy over a fixed example set. Every field's examples are
/// cycled, so the catalog covers each example of each field at least once
/// without a combinatorial blow-up.
pub fn examples_strategy<T: Clone + std::fmt::Debug + 'static>(
    examples: Vec<T>,
) -> BoxedStrategy<T> {
    assert!(
        !examples.is_empty(),
        "step examples must be non-empty — the round-trip property would be vacuous"
    );
    proptest::sample::select(examples).boxed()
}

/// The number of examples a struct produces: the longest field example list,
/// so every example of every field is exercised.
pub fn example_count(field_example_counts: &[usize]) -> usize {
    field_example_counts
        .iter()
        .copied()
        .max()
        .unwrap_or(1)
        .max(1)
}

// ── StepField impls for the shared field types ────────────────────

macro_rules! step_field_bare {
    ($ty:ty, $examples:expr) => {
        impl StepField for $ty {
            const QUOTED: bool = false;
            fn render_step_field(&self) -> String {
                self.to_string()
            }
            fn parse_step_field(raw: &str) -> Result<Self, String> {
                raw.trim()
                    .parse::<$ty>()
                    .map_err(|e| format!("{:?} is not a valid {}: {e}", raw, stringify!($ty)))
            }
            fn step_field_examples() -> Vec<Self> {
                $examples
            }
        }
    };
}

step_field_bare!(bool, vec![false, true]);
step_field_bare!(u8, vec![0, 1, 7]);
step_field_bare!(i32, vec![-3, 0, 42]);
step_field_bare!(i64, vec![-1, 0, 365]);
step_field_bare!(usize, vec![0, 1, 10]);

impl StepField for String {
    const QUOTED: bool = true;
    fn render_step_field(&self) -> String {
        self.clone()
    }
    fn parse_step_field(raw: &str) -> Result<Self, String> {
        Ok(raw.to_string())
    }
    fn step_field_examples() -> Vec<Self> {
        vec![
            String::new(),
            "hello".to_string(),
            // Deliberately adversarial: quotes, backslashes, and a fragment
            // that looks like another template's literal segment.
            r#"a "quoted" \ in region "main""#.to_string(),
        ]
    }
}

impl StepField for holon_api::EntityUri {
    const QUOTED: bool = true;
    fn render_step_field(&self) -> String {
        self.to_string()
    }
    fn parse_step_field(raw: &str) -> Result<Self, String> {
        if raw.is_empty() {
            return Err("an entity uri must not be empty".to_string());
        }
        // ALLOW(entity_uri_from_raw): step text is the test DSL boundary.
        Ok(holon_api::EntityUri::from_raw(raw))
    }
    fn step_field_examples() -> Vec<Self> {
        vec![
            holon_api::EntityUri::block("blk-a"),
            holon_api::EntityUri::block("blk-b"),
        ]
    }
}

impl StepField for holon_api::Region {
    const QUOTED: bool = true;
    fn render_step_field(&self) -> String {
        match self {
            holon_api::Region::Main => "main",
            holon_api::Region::LeftSidebar => "left_sidebar",
            holon_api::Region::RightSidebar => "right_sidebar",
        }
        .to_string()
    }
    fn parse_step_field(raw: &str) -> Result<Self, String> {
        match raw.to_lowercase().as_str() {
            "main" => Ok(holon_api::Region::Main),
            "left" | "left_sidebar" | "leftsidebar" => Ok(holon_api::Region::LeftSidebar),
            "right" | "right_sidebar" | "rightsidebar" => Ok(holon_api::Region::RightSidebar),
            other => Err(format!("unknown region: {other:?}")),
        }
    }
    fn step_field_examples() -> Vec<Self> {
        vec![
            holon_api::Region::Main,
            holon_api::Region::LeftSidebar,
            holon_api::Region::RightSidebar,
        ]
    }
}

/// Payload types with no readable one-line spelling travel as compact JSON
/// inside a quoted run. Round-trip is serde's, so it is lossless by
/// construction.
#[macro_export]
macro_rules! step_field_via_json {
    ($ty:ty, $examples:expr) => {
        impl $crate::step_vocabulary::StepField for $ty {
            const QUOTED: bool = true;
            fn render_step_field(&self) -> String {
                ::serde_json::to_string(self).unwrap_or_else(|e| {
                    panic!(
                        "{} is not serializable as a step field: {e}",
                        stringify!($ty)
                    )
                })
            }
            fn parse_step_field(raw: &str) -> Result<Self, String> {
                ::serde_json::from_str(raw)
                    .map_err(|e| format!("{:?} is not a valid {}: {e}", raw, stringify!($ty)))
            }
            fn step_field_examples() -> Vec<Self> {
                $examples
            }
        }
    };
}

/// Enums whose serde form is a single string spell that string directly, so a
/// recorded step reads `to state "TODO"` rather than a nested JSON literal.
#[macro_export]
macro_rules! step_field_via_serde_string {
    ($ty:ty, $examples:expr) => {
        impl $crate::step_vocabulary::StepField for $ty {
            const QUOTED: bool = true;
            fn render_step_field(&self) -> String {
                match ::serde_json::to_value(self) {
                    Ok(::serde_json::Value::String(s)) => s,
                    other => panic!(
                        "{} must serialize to a JSON string to render as one, got {:?}",
                        stringify!($ty),
                        other
                    ),
                }
            }
            fn parse_step_field(raw: &str) -> Result<Self, String> {
                ::serde_json::from_value(::serde_json::Value::String(raw.to_string()))
                    .map_err(|e| format!("{:?} is not a valid {}: {e}", raw, stringify!($ty)))
            }
            fn step_field_examples() -> Vec<Self> {
                $examples
            }
        }
    };
}

step_field_via_json!(
    holon_api::QueryLanguage,
    vec![
        holon_api::QueryLanguage::HolonPrql,
        holon_api::QueryLanguage::HolonSql
    ]
);
step_field_via_serde_string!(
    crate::types::CycleTarget,
    crate::types::CycleTarget::ALL.to_vec()
);
step_field_via_json!(
    crate::types::LoroCorruptionType,
    vec![
        crate::types::LoroCorruptionType::Empty,
        crate::types::LoroCorruptionType::Truncated,
        crate::types::LoroCorruptionType::InvalidHeader,
    ]
);
step_field_via_json!(
    crate::types::MutationEvent,
    vec![crate::types::MutationEvent {
        source: crate::types::MutationSource::UI,
        mutation: crate::types::Mutation::Delete {
            id: holon_api::EntityUri::block("blk-a"),
        },
    }]
);
step_field_via_json!(
    crate::capabilities::TextOp,
    vec![
        crate::capabilities::TextOp::Insert {
            pos_codepoint: 0,
            text: "hi".to_string(),
        },
        crate::capabilities::TextOp::Delete {
            pos_codepoint: 1,
            len_codepoint: 2,
        },
    ]
);
step_field_via_json!(
    crate::capabilities::PeerEditOp,
    vec![
        crate::capabilities::PeerEditOp::Delete {
            stable_id: "peer-blk".to_string(),
        },
        crate::capabilities::PeerEditOp::Update {
            stable_id: "peer-blk".to_string(),
            content: "text".to_string(),
        },
    ]
);
step_field_via_json!(
    holon_api::EdgeFieldUpdate,
    vec![holon_api::EdgeFieldUpdate::Requires(vec![
        holon_api::EntityUri::block("blk-b"),
    ])]
);
step_field_via_json!(
    holon_api::KeyChord,
    vec![
        holon_api::KeyChord::new(&[holon_api::Key::Enter]),
        holon_api::KeyChord::new(&[holon_api::Key::Cmd, holon_api::Key::Enter]),
    ]
);
step_field_via_json!(
    Option<holon_api::EntityUri>,
    vec![None, Some(holon_api::EntityUri::block("blk-a"))]
);
/// `Block` is deliberately not `Serialize` (the wire form is `BlockWire`), so
/// a block payload travels as its wire JSON.
#[derive(serde::Serialize, serde::Deserialize)]
struct BlocksWire(#[serde(with = "holon_api::block::block_wire_vec")] Vec<holon_api::Block>);

impl StepField for Vec<holon_api::Block> {
    const QUOTED: bool = true;
    fn render_step_field(&self) -> String {
        serde_json::to_string(&BlocksWire(self.clone()))
            .unwrap_or_else(|e| panic!("blocks are not serializable as a step field: {e}"))
    }
    fn parse_step_field(raw: &str) -> Result<Self, String> {
        serde_json::from_str::<BlocksWire>(raw)
            .map(|w| w.0)
            .map_err(|e| format!("{raw:?} is not a valid block list: {e}"))
    }
    fn step_field_examples() -> Vec<Self> {
        vec![
            Vec::new(),
            vec![holon_api::Block::new_text(
                holon_api::EntityUri::block("blk-a"),
                holon_api::EntityUri::block("doc-a"),
                "content".to_string(),
            )],
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_fields_survive_quotes_and_backslashes() {
        let template = "I type {text}";
        let raw = r#"say "hi" \ then stop"#.to_string();
        let text = render_template(template, &[("text", true, raw.clone())]);
        let caps = capture_template(template, &[("text", true)], &text).expect("must match");
        assert_eq!(captured(&caps, "text"), raw);
    }

    #[test]
    fn a_quoted_field_cannot_impersonate_a_literal_segment() {
        let template = "I click block {block_id} in region {region}";
        let fields: &[TemplateField] = &[("block_id", true), ("region", true)];
        let text = render_template(
            template,
            &[
                (
                    "block_id",
                    true,
                    r#"blk" in region "left_sidebar"#.to_string(),
                ),
                ("region", true, "main".to_string()),
            ],
        );
        let caps = capture_template(template, fields, &text).expect("must match");
        assert_eq!(captured(&caps, "region"), "main");
        assert_eq!(
            captured(&caps, "block_id"),
            r#"blk" in region "left_sidebar"#
        );
    }

    #[test]
    fn bare_fields_stop_at_the_next_literal() {
        let template = "I split block {id} at position {pos}";
        let fields: &[TemplateField] = &[("id", true), ("pos", false)];
        let caps = capture_template(template, fields, r#"I split block "blk-a" at position 10"#)
            .expect("must match");
        assert_eq!(captured(&caps, "pos"), "10");
    }

    #[test]
    fn a_non_matching_template_reports_no_match() {
        let template = "I indent block {id}";
        let fields: &[TemplateField] = &[("id", true)];
        assert!(capture_template(template, fields, r#"I outdent block "blk-a""#).is_none());
    }

    #[test]
    fn identical_skeletons_are_refused() {
        let err = check_template_ambiguity(&[
            ("A", "I poke block {id}"),
            ("B", "I poke block {block_id}"),
        ])
        .expect_err("identical skeletons must be refused");
        assert!(err.contains("identical literal skeletons"), "{err}");
    }

    #[test]
    fn templates_differing_only_in_a_trailing_literal_pass() {
        check_template_ambiguity(&[
            ("MoveUp", "I move block {id} up"),
            ("MoveDown", "I move block {id} down"),
        ])
        .expect("a distinguishing trailing literal is enough");
    }

    #[test]
    fn a_leading_prefix_at_equal_arity_is_refused() {
        let err = check_template_ambiguity(&[("A", "I poke {id}"), ("B", "I poke block {id}")])
            .expect_err("prefix at equal arity must be refused");
        assert!(err.contains("prefix-compatible"), "{err}");
    }

    #[test]
    fn distinct_skeletons_pass() {
        check_template_ambiguity(&[
            ("A", "I indent block {id}"),
            ("B", "I outdent block {id}"),
            ("C", "I split block {id} at position {pos}"),
        ])
        .expect("distinct skeletons must pass");
    }
}
