//! LogSeq's `built-in-entity?` rule, stated once.
//!
//! Two readers ask it — the writer over the B+-trees it is about to edit
//! (`TreeDatom`s) and the importer over the datoms it decoded
//! (`LogseqDatom`s). They read different structures, so what they share is
//! this module's [`Marker`]: the two facts about a datom the rule depends on,
//! with each reader responsible only for producing them.

use crate::DatomValue;
use crate::LogseqAttr;
use crate::TransitNode;
use crate::tree::TreeDatom;

/// LogSeq's marker for the property and class pages it ships with.
const BUILT_IN_FLAG: &str = "logseq.property/built-in?";
const FILE_PATH: &str = "file/path";
const DB_IDENT: &str = "db/ident";

/// The part of a datom's value the rule can see: `true`, a keyword, or
/// neither. Everything else a value may be answers the rule identically, so
/// widening this enum would only invite a fourth arm that means nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkerValue<'a> {
    True,
    Keyword(&'a str),
    Other,
}

/// One datom, reduced to what the built-in rule reads.
///
/// `attribute` is the ident WITHOUT its leading colon — the writer's trees
/// store it that way and the importer strips it, so the legs below match one
/// spelling rather than two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Marker<'a> {
    pub attribute: &'a str,
    pub value: MarkerValue<'a>,
}

impl<'a> From<&'a TreeDatom> for Marker<'a> {
    fn from(datom: &'a TreeDatom) -> Self {
        Self {
            attribute: &datom.a,
            value: MarkerValue::of_node(&datom.v),
        }
    }
}

impl<'a> From<&'a crate::LogseqDatom> for Marker<'a> {
    fn from(datom: &'a crate::LogseqDatom) -> Self {
        Self {
            attribute: attribute_name(&datom.a),
            value: match &datom.v {
                DatomValue::Node(node) => MarkerValue::of_node(node),
                DatomValue::Ref(_) => MarkerValue::Other,
            },
        }
    }
}

impl<'a> MarkerValue<'a> {
    fn of_node(node: &'a TransitNode) -> Self {
        match node {
            TransitNode::Bool(true) => Self::True,
            TransitNode::Keyword(name) => Self::Keyword(name),
            _ => Self::Other,
        }
    }
}

/// The importer's attribute ident, without its leading colon.
fn attribute_name(attribute: &LogseqAttr) -> &str {
    attribute.ident().trim_start_matches(':')
}

/// Whether this one datom makes its entity one of LogSeq's own.
///
/// Three legs, because LogSeq's `outliner-validate/built-in-entity?` has
/// three and says in its own docstring that the flag alone is not enough:
/// the flag, OR a `:file/path` (config.edn, custom.css and friends carry no
/// flag at all), OR an internal `:db/ident` (the `:logseq.kv/*` entries).
///
/// An ENTITY is built-in when any datom of its makes it so; this decides one
/// datom, which is the part both readers can express.
pub(crate) fn marks_built_in(marker: Marker<'_>) -> bool {
    match (marker.attribute, marker.value) {
        (BUILT_IN_FLAG, MarkerValue::True) => true,
        (FILE_PATH, _) => true,
        (DB_IDENT, MarkerValue::Keyword(name)) => is_internal_ident(name),
        _ => false,
    }
}

/// Whether `keyword` is an ident LogSeq minted for itself.
///
/// A MEASURED APPROXIMATION of LogSeq's `internal-ident?`, not a restatement
/// of it. LogSeq's is a MEMBERSHIP test over the idents it declares plus the
/// ones it creates at runtime — same namespace, opposite verdicts:
/// `:block/title` is internal, `:block/uuid` is not; `:logseq.kv/*` is,
/// `:logseq/foo` is not.
///
/// Exact membership was tried and REJECTED by measurement: LogSeq's DECLARED
/// schema set (146 idents) misses 35 of this graph's 171 — every
/// `:logseq.kv/*` and every closed-value ident, all created at runtime —
/// including all eight entities that are built-in by this leg alone. Matching
/// on the declared set would make those PUSHABLE, which is the under-refusing
/// direction and the one that loses data.
///
/// So a namespace rule stays, and BOTH namespaces are here because the
/// alternative is worse in the direction that matters. LogSeq calls 13 of the
/// 16 declared `block/*` idents internal; this arm over-refuses the other
/// three (`:block/name`, `:block/tx-id`, `:block/uuid`). Dropping the arm to
/// avoid those three would instead UNDER-refuse the 13 — and the predicate
/// this feeds is `pub`, so a caller reaching an entity push cannot reach would
/// get the permissive answer with nothing to warn them. Fail closed:
/// over-refusing an edit is visible, under-refusing one rewrites LogSeq's
/// schema silently.
///
/// No base-reachable entity is built-in by a `block/*` ident ALONE (measured —
/// every such entity in this graph also carries the flag, so leg 1 answers
/// first), so push does not exercise this arm today. That makes it a guard
/// against future reach, not a live code path, and it is drivable only by a
/// constructed entity — which is exactly how the tests drive it.
///
/// All nine divergences are OVER-refusals: the three `block/*` above and six
/// third-party namespaces merely beginning with "logseq" (`:logseq/foo`,
/// `:logseqfoo/bar`, `:logseq-plugin/x`, `:logseq.thirdparty/x`,
/// `:logseq_x/y`, `:logseqified/x`). 182 of 191 measured idents agree exactly.
/// Pinned ident by ident, with the divergence set asserted as over-refusal, in
/// `tests/fixtures/logseq-db/internal-ident-reference.json`.
pub(crate) fn is_internal_ident(keyword: &str) -> bool {
    let namespace = keyword.split('/').next().unwrap_or("");
    namespace == "block" || namespace.starts_with("logseq")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Eid;
    use crate::LogseqDatom;

    /// The two readers reach the same verdict because they reach the same
    /// function — this pins that the CONVERSIONS agree, which is the only
    /// part either reader still states for itself.
    #[test]
    fn the_two_datom_representations_produce_the_same_marker() {
        let cases: Vec<(LogseqAttr, TransitNode)> = vec![
            (
                LogseqAttr::Raw(":logseq.property/built-in?".to_string()),
                TransitNode::Bool(true),
            ),
            (
                LogseqAttr::Raw(":logseq.property/built-in?".to_string()),
                TransitNode::Bool(false),
            ),
            (
                LogseqAttr::Raw(":file/path".to_string()),
                TransitNode::Str("config.edn".to_string()),
            ),
            (
                LogseqAttr::DbIdent,
                TransitNode::Keyword("logseq.kv/graph-uuid".to_string()),
            ),
            (
                LogseqAttr::DbIdent,
                TransitNode::Keyword("my.plugin/thing".to_string()),
            ),
            (LogseqAttr::Title, TransitNode::Str("plain".to_string())),
        ];
        for (attribute, value) in cases {
            let tree = TreeDatom {
                e: 1,
                a: attribute.ident().trim_start_matches(':').to_string(),
                v: value.clone(),
                tx: 1,
            };
            let logseq = LogseqDatom {
                e: Eid(1),
                a: attribute.clone(),
                v: DatomValue::Node(value.clone()),
                tx: None,
            };
            assert_eq!(
                Marker::from(&tree),
                Marker::from(&logseq),
                "{attribute:?} = {value:?} must reduce identically"
            );
        }
    }

    /// A ref-valued datom can never be a marker, and the importer is the only
    /// reader that can express one.
    #[test]
    fn a_ref_value_marks_nothing() {
        let datom = LogseqDatom {
            e: Eid(1),
            a: LogseqAttr::Raw(":logseq.property/built-in?".to_string()),
            v: DatomValue::Ref(Eid(2)),
            tx: None,
        };
        assert!(!marks_built_in(Marker::from(&datom)));
    }
}
