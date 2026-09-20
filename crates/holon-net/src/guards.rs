//! Guard classification, on two independent axes.
//!
//! Axis one, predicate expressibility. A conjunct whose shape the arc language
//! carries becomes an arc: a subject column, name, property or tag test
//! against a literal becomes a refined read arc, and a `parent(…)` /
//! `child(…)` / `sibling(…)` hop becomes a [`CorrelatedGroup`] whose arcs are
//! all tested against one bound token. Everything else stays an opaque
//! [`GuardResidue`], never lossily approximated into an arc.
//!
//! A disjunction is not approximated either: the body is put into disjunctive
//! normal form and each disjunct becomes a [`TransitionMode`]. Disjunction
//! lifts out of a hop (`child(a or b)` is `child(a) or child(b)`, because an
//! existential distributes over a disjunction) but conjunction does NOT, which
//! is why a hop's conjuncts share one binding.
//!
//! Negation is NOT pushed to the leaves — there is no De Morgan pass here, so
//! a negated conjunct stays residue whatever it negates. That pass and the
//! inhibitor it feeds are the next increment's; until then the census counts
//! negation as its own residue cause.
//!
//! Axis two, place footprint: a guard reads every place it tests, whatever its
//! predicate. The grammar is closed, so the footprint is extractable for every
//! guard, and residue conjuncts contribute [`ArcOrigin::GuardFootprint`] read
//! arcs. Dropping those reads would under-approximate the read set and lose
//! real conflict and cycle edges.

use std::collections::BTreeSet;

use holon_pattern::arcs::ArcPlace;
use holon_pattern::pattern::BuiltinRef;
use holon_pattern::pattern::FieldRef;
use holon_pattern::pattern::Guard;
use holon_pattern::pattern::Operand;
use holon_pattern::pattern::PathSegment;
use holon_pattern::pattern::Pattern;
use holon_pattern::pattern::Subject;
use holon_pattern::schema;
use holon_pattern::schema::block;
use holon_pattern::schema::clock;

use crate::net::ArcOrigin;
use crate::net::BindingVar;
use crate::net::CorrelatedGroup;
use crate::net::Correlation;
use crate::net::Flow;
use crate::net::GuardResidue;
use crate::net::Hop;
use crate::net::MAX_HOP_CHAIN;
use crate::net::MAX_MODES;
use crate::net::NetArc;
use crate::net::NetCompileError;
use crate::net::Refinement;
use crate::net::TransitionMode;

/// A classified guard: one [`TransitionMode`] per disjunct of the body in
/// disjunctive normal form, plus the conjuncts no mode could express.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassifiedGuard {
    pub modes: Vec<TransitionMode>,
    pub residue: Vec<GuardResidue>,
}

/// Why a residue conjunct could not become an arc.
///
/// Reported per cause so the census measures which language gap is costing
/// what, rather than one opaque total.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ResidueCause {
    /// A negation. No De Morgan pass exists yet, so `not` is opaque whatever
    /// it wraps; `not block_exists(…)` additionally needs the inhibitor.
    Negation,
    /// An existence test over a path, which names no single place's token.
    Existence,
    /// A comparison against a builtin such as `{today}` rather than a literal.
    Builtin,
    /// A hop whose body the arc language cannot refine.
    UnrefinableHop,
    /// The guard's disjunctive normal form exceeded [`MAX_MODES`]. Only a
    /// user-authored rule reaches this as residue; a shipped guard fails the
    /// build.
    TooManyModes,
    /// The guard's hop chain exceeded [`MAX_HOP_CHAIN`], same split.
    HopChainTooLong,
    /// Anything else the grammar grows.
    Other,
}

impl ResidueCause {
    pub fn of(pattern: &Pattern) -> ResidueCause {
        match pattern {
            Pattern::Not(_) => ResidueCause::Negation,
            Pattern::BlockExists(_) => ResidueCause::Existence,
            Pattern::Field {
                rhs: Operand::Builtin(_),
                ..
            } => ResidueCause::Builtin,
            Pattern::Parent(_) | Pattern::Child(_) | Pattern::Sibling(_) => {
                ResidueCause::UnrefinableHop
            }
            _ => ResidueCause::Other,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ResidueCause::Negation => "negation",
            ResidueCause::Existence => "existence",
            ResidueCause::Builtin => "builtin",
            ResidueCause::UnrefinableHop => "unrefinable_hop",
            ResidueCause::TooManyModes => "too_many_modes",
            ResidueCause::HopChainTooLong => "hop_chain_too_long",
            ResidueCause::Other => "other",
        }
    }
}

/// Classify `guard` into firing modes and residue. Total over the shapes it
/// accepts: every conjunct of every disjunct lands in exactly one of the two,
/// and every place the guard names appears on an arc.
///
/// # Errors
/// [`NetCompileError::TooManyModes`] and
/// [`NetCompileError::HopChainTooLong`] when the guard exceeds a bound. These
/// are loud on purpose: silently leaving an over-budget guard as residue would
/// tell its author nothing about why their predicate stopped being analyzable.
/// `transition` names the guard in the message.
pub fn classify_guard(guard: &Guard, transition: &str) -> Result<ClassifiedGuard, NetCompileError> {
    let subject_arc = read_arc(subject_place(&guard.subject), ArcOrigin::Subject, None);
    let disjuncts = disjuncts_of(&guard.body, transition)?;
    if disjuncts.len() > MAX_MODES {
        return Err(NetCompileError::TooManyModes {
            transition: transition.to_string(),
            modes: disjuncts.len(),
            limit: MAX_MODES,
        });
    }

    let mut modes = Vec::with_capacity(disjuncts.len());
    let mut residue = Vec::new();
    for conjuncts in disjuncts {
        let mut arcs = vec![subject_arc.clone()];
        let mut hops = Vec::new();
        for conjunct in conjuncts {
            match classify_conjunct(&conjunct, hops.len(), transition)? {
                Classified::Refined(arc) => arcs.push(arc),
                Classified::Hop(group) => {
                    // The places the hop TRAVERSES are read just as surely as
                    // the cells it tests. Leaving them off would
                    // under-approximate the read set and lose conflict and
                    // cycle edges.
                    for hop in &group.correlation.hops {
                        for place in [hop.from.clone(), hop.to.clone()] {
                            arcs.push(read_arc(place, ArcOrigin::GuardHop, None));
                        }
                    }
                    if group.correlation.exclude_subject {
                        // Excluding the subject compares identities, so the
                        // identity place is read too.
                        arcs.push(read_arc(
                            ArcPlace::new(block::RELATION, block::ID),
                            ArcOrigin::GuardHop,
                            None,
                        ));
                    }
                    hops.push(group);
                }
                Classified::Opaque => {
                    for place in pattern_footprint(&conjunct) {
                        arcs.push(read_arc(place, ArcOrigin::GuardFootprint, None));
                    }
                    let entry = GuardResidue {
                        cause: ResidueCause::of(&conjunct),
                        predicate: conjunct.clone(),
                    };
                    if !residue.contains(&entry) {
                        residue.push(entry);
                    }
                }
            }
        }
        dedupe(&mut arcs);
        modes.push(TransitionMode { arcs, hops });
    }
    Ok(ClassifiedGuard { modes, residue })
}

enum Classified {
    Refined(NetArc),
    Hop(CorrelatedGroup),
    Opaque,
}

fn classify_conjunct(
    conjunct: &Pattern,
    index: usize,
    transition: &str,
) -> Result<Classified, NetCompileError> {
    if let Some((place, refinement)) = refinement_of(conjunct) {
        return Ok(Classified::Refined(read_arc(
            place,
            ArcOrigin::GuardRefinement,
            Some(refinement),
        )));
    }
    Ok(match correlated_group(conjunct, index, transition)? {
        Some(group) => Classified::Hop(group),
        None => Classified::Opaque,
    })
}

/// Peel hop wrappers into a chain, then refine the innermost conjunction
/// against the bound token.
///
/// `Ok(None)` when the innermost body is not a pure conjunction of refinable
/// leaves, or when the conjunct is not a hop at all — both are honest
/// "cannot express", which the offer path answers `Unknown` for.
///
/// # Errors
/// [`NetCompileError::HopChainTooLong`] past [`MAX_HOP_CHAIN`].
fn correlated_group(
    conjunct: &Pattern,
    index: usize,
    transition: &str,
) -> Result<Option<CorrelatedGroup>, NetCompileError> {
    let mut hops = Vec::new();
    let mut exclude_subject = false;
    let mut inner = conjunct;
    loop {
        let (hop, excludes, next) = match inner {
            Pattern::Parent(next) => (Hop::parent(), false, next.as_ref()),
            Pattern::Child(next) => (Hop::child(), false, next.as_ref()),
            Pattern::Sibling(next) => (Hop::sibling(), true, next.as_ref()),
            _ => break,
        };
        hops.push(hop);
        exclude_subject |= excludes;
        if hops.len() > MAX_HOP_CHAIN {
            return Err(NetCompileError::HopChainTooLong {
                transition: transition.to_string(),
                depth: hop_depth(conjunct),
                limit: MAX_HOP_CHAIN,
            });
        }
        inner = next;
    }
    if hops.is_empty() {
        return Ok(None);
    }

    let binding = BindingVar::new(format!("hop{index}"));
    let mut arcs = Vec::new();
    for leaf in conjuncts_of(inner) {
        let Some((place, refinement)) = refinement_of(leaf) else {
            return Ok(None);
        };
        arcs.push(NetArc {
            place,
            flow: Flow::Read,
            origin: ArcOrigin::GuardHop,
            refinement: Some(refinement),
            binding: Some(binding.clone()),
        });
    }
    // A hop with nothing to test is `parent(1)` — the arc language has no way
    // to say "a parent exists" without naming a place, so it is not an arc.
    if arcs.is_empty() {
        return Ok(None);
    }
    dedupe(&mut arcs);
    Ok(Some(CorrelatedGroup {
        binding,
        correlation: Correlation {
            hops,
            exclude_subject,
        },
        arcs,
    }))
}

/// How many hops a conjunct nests, for the error message.
fn hop_depth(pattern: &Pattern) -> usize {
    match pattern {
        Pattern::Parent(inner) | Pattern::Child(inner) | Pattern::Sibling(inner) => {
            1 + hop_depth(inner)
        }
        _ => 0,
    }
}

/// Disjunctive normal form, one conjunct list per disjunct.
///
/// Disjunction lifts out of a hop because an existential distributes over it;
/// conjunction does not, so a hop's conjunction is carried into the hop whole.
/// Negation is opaque: no De Morgan pass exists yet.
///
/// # Errors
/// [`NetCompileError::TooManyModes`] as soon as the cross-product passes the
/// bound. Returning a partial product instead would be a strictly weaker
/// predicate escaping into the net.
fn disjuncts_of(pattern: &Pattern, transition: &str) -> Result<Vec<Vec<Pattern>>, NetCompileError> {
    let out = match pattern {
        Pattern::And(ps) => {
            let mut out: Vec<Vec<Pattern>> = vec![Vec::new()];
            for p in ps {
                let mut next = Vec::new();
                for prefix in &out {
                    for disjunct in disjuncts_of(p, transition)? {
                        let mut combined = prefix.clone();
                        combined.extend(disjunct);
                        next.push(combined);
                    }
                }
                out = next;
            }
            out
        }
        Pattern::Or(ps) => {
            let mut out = Vec::new();
            for p in ps {
                out.extend(disjuncts_of(p, transition)?);
            }
            out
        }
        Pattern::Parent(inner) => lift_hop(inner, Pattern::Parent, transition)?,
        Pattern::Child(inner) => lift_hop(inner, Pattern::Child, transition)?,
        Pattern::Sibling(inner) => lift_hop(inner, Pattern::Sibling, transition)?,
        other => vec![vec![other.clone()]],
    };
    if out.len() > MAX_MODES {
        return Err(NetCompileError::TooManyModes {
            transition: transition.to_string(),
            modes: out.len(),
            limit: MAX_MODES,
        });
    }
    Ok(out)
}

/// `hop(a or b)` becomes `hop(a) or hop(b)`; each disjunct's conjunction stays
/// inside its own hop.
fn lift_hop(
    inner: &Pattern,
    wrap: fn(Box<Pattern>) -> Pattern,
    transition: &str,
) -> Result<Vec<Vec<Pattern>>, NetCompileError> {
    Ok(disjuncts_of(inner, transition)?
        .into_iter()
        .map(|conjuncts| vec![wrap(Box::new(and_of(conjuncts)))])
        .collect())
}

fn and_of(mut conjuncts: Vec<Pattern>) -> Pattern {
    if conjuncts.len() == 1 {
        conjuncts.remove(0)
    } else {
        Pattern::And(conjuncts)
    }
}

/// Top-level conjuncts: nested `And`s flattened, anything else one conjunct.
fn conjuncts_of(pattern: &Pattern) -> Vec<&Pattern> {
    match pattern {
        Pattern::And(ps) => ps.iter().flat_map(conjuncts_of).collect(),
        other => vec![other],
    }
}

/// Every place `pattern` syntactically tests. One mapping, stated once:
///
/// - a field comparison → the field's place (`name` → `block.content`,
///   `property` → `block.properties`, a relation column → itself)
/// - a `{today}` operand or path segment → `clock.today`
/// - `has_tag` → `block.tags`
/// - `block_exists` → `block.id`, `block.content`, plus `block.parent_id` when
///   the path has ancestors
/// - a hop → `block.parent_id` and `block.id`, the places it traverses, plus
///   `inner`'s footprint
pub fn pattern_footprint(pattern: &Pattern) -> BTreeSet<ArcPlace> {
    let mut out = BTreeSet::new();
    walk_footprint(pattern, &mut out);
    out
}

fn walk_footprint(pattern: &Pattern, out: &mut BTreeSet<ArcPlace>) {
    match pattern {
        Pattern::Field { field, rhs, .. } => {
            out.insert(field_place(field));
            if let Operand::Builtin(b) = rhs {
                out.insert(builtin_place(b));
            }
        }
        Pattern::HasTag(_) => {
            out.insert(ArcPlace::new(block::RELATION, block::TAGS));
        }
        Pattern::BlockExists(path) => {
            out.insert(ArcPlace::new(block::RELATION, block::ID));
            out.insert(ArcPlace::new(block::RELATION, block::CONTENT));
            if path.segments.len() > 1 {
                out.insert(ArcPlace::new(block::RELATION, block::PARENT_ID));
            }
            for segment in &path.segments {
                if let PathSegment::Builtin(b) = segment {
                    out.insert(builtin_place(b));
                }
            }
        }
        Pattern::Parent(inner) | Pattern::Child(inner) | Pattern::Sibling(inner) => {
            out.insert(ArcPlace::new(block::RELATION, block::PARENT_ID));
            out.insert(ArcPlace::new(block::RELATION, block::ID));
            walk_footprint(inner, out);
        }
        Pattern::And(ps) | Pattern::Or(ps) => {
            for p in ps {
                walk_footprint(p, out);
            }
        }
        Pattern::Not(inner) => walk_footprint(inner, out),
    }
}

/// The place and typed predicate an expressible leaf refines, or `None`. The
/// expressible shapes are a subject attribute against a literal (a relation
/// column, the name, a property under its key) and tag membership; a clock
/// operand, an existence test, negation and disjunction are not.
fn refinement_of(conjunct: &Pattern) -> Option<(ArcPlace, Refinement)> {
    match conjunct {
        Pattern::Field {
            field: FieldRef::Property(key),
            op,
            rhs: Operand::Lit(value),
        } => Some((
            ArcPlace::new(block::RELATION, block::PROPERTIES),
            Refinement::Keyed {
                key: key.clone(),
                op: *op,
                rhs: value.clone(),
            },
        )),
        Pattern::Field {
            field,
            op,
            rhs: Operand::Lit(value),
        } => Some((
            field_place(field),
            Refinement::Cell {
                op: *op,
                rhs: value.clone(),
            },
        )),
        Pattern::HasTag(tag) => Some((
            ArcPlace::new(block::RELATION, block::TAGS),
            Refinement::HasTag(tag.clone()),
        )),
        _ => None,
    }
}

fn field_place(field: &FieldRef) -> ArcPlace {
    match field {
        FieldRef::Name => ArcPlace::new(block::RELATION, block::CONTENT),
        FieldRef::Property(_) => ArcPlace::new(block::RELATION, block::PROPERTIES),
        FieldRef::Column { relation, name } => ArcPlace::new(relation.clone(), name.clone()),
    }
}

fn builtin_place(builtin: &BuiltinRef) -> ArcPlace {
    match builtin {
        BuiltinRef::Today => ArcPlace::new(clock::RELATION, clock::TODAY),
    }
}

/// The binding row a guard's subject iterates.
fn subject_place(subject: &Subject) -> ArcPlace {
    match subject {
        Subject::Clock => ArcPlace::new(clock::RELATION, clock::TODAY),
        Subject::Block => ArcPlace::new(block::RELATION, block::ID),
        Subject::Relation(relation) => {
            let id_column = schema::builtin_entity(relation)
                .and_then(|e| e.binding)
                .map(|b| b.id_column)
                .unwrap_or("id");
            ArcPlace::new(relation.clone(), id_column)
        }
    }
}

fn read_arc(place: ArcPlace, origin: ArcOrigin, refinement: Option<Refinement>) -> NetArc {
    NetArc {
        place,
        flow: Flow::Read,
        origin,
        refinement,
        binding: None,
    }
}

fn dedupe(arcs: &mut Vec<NetArc>) {
    let mut seen = Vec::new();
    arcs.retain(|arc| {
        if seen.contains(arc) {
            false
        } else {
            seen.push(arc.clone());
            true
        }
    });
}
#[cfg(test)]
mod tests {
    use holon_pattern::Value;
    use holon_pattern::pattern::CmpOp;
    use holon_pattern::pattern::Guard;

    use super::*;
    use crate::net::NetCompileError;

    /// A block-column comparison, built as an AST.
    ///
    /// The guard TEXT surface cannot spell this beside a block predicate: a
    /// `relation.column` comparison makes the subject that relation, and
    /// mixing one with `parent`/`child`/`has_tag` is refused at parse
    /// (`GuardParseError::RelationAndBlockPredicate`). Compiling such a guard
    /// is nonetheless well defined, and the catalog builds its guards through
    /// the macro rather than through the text parser.
    fn col(name: &str, value: &str) -> Pattern {
        Pattern::Field {
            field: FieldRef::Column {
                relation: block::RELATION.to_string(),
                name: name.to_string(),
            },
            op: CmpOp::Eq,
            rhs: Operand::Lit(Value::String(value.to_string())),
        }
    }

    fn block_guard(body: Pattern) -> Guard {
        Guard {
            subject: Subject::Block,
            body,
        }
    }

    fn only_mode(classified: &ClassifiedGuard) -> &TransitionMode {
        assert_eq!(
            classified.modes.len(),
            1,
            "expected one firing mode, got {}",
            classified.modes.len()
        );
        &classified.modes[0]
    }

    fn places(mode: &TransitionMode, origin: ArcOrigin) -> Vec<String> {
        mode.arcs
            .iter()
            .filter(|a| a.origin == origin)
            .map(|a| a.place.to_string())
            .collect()
    }

    /// The production `integration.begin_oauth` guard: three subject-column
    /// equalities, all expressible, one mode.
    #[test]
    fn a_conjoined_column_guard_is_all_refinements() {
        let guard = Guard::parse(
            "integration.config_status == \"unconfigured\" and integration.configurable == 1 \
             and integration.configure_progress == \"\"",
        )
        .unwrap();
        let classified = classify_guard(&guard, "op:block.probe").expect("within the bounds");
        assert!(classified.residue.is_empty(), "{:?}", classified.residue);
        let mode = only_mode(&classified);
        assert_eq!(
            places(mode, ArcOrigin::GuardRefinement),
            [
                "integration.config_status",
                "integration.configurable",
                "integration.configure_progress"
            ]
        );
        assert!(
            mode.arcs
                .iter()
                .filter(|a| a.origin == ArcOrigin::GuardRefinement)
                .all(|a| a.refinement.is_some())
        );
    }

    #[test]
    fn a_tag_guard_is_one_refinement_on_the_tags_place() {
        let guard = Guard::parse("has_tag(\"flaggable\")").unwrap();
        let classified = classify_guard(&guard, "op:block.probe").expect("within the bounds");
        assert!(classified.residue.is_empty());
        assert_eq!(
            places(only_mode(&classified), ArcOrigin::GuardRefinement),
            ["block.tags"]
        );
    }

    /// The `child` keyword reaches the AST through the text parser, beside
    /// the `parent` it mirrors.
    #[test]
    fn the_child_keyword_parses() {
        let guard = Guard::parse("child(has_tag(\"Page\"))").unwrap();
        assert!(matches!(guard.body, Pattern::Child(_)), "{:?}", guard.body);
    }

    /// The parent hop is an arc now, not residue: a correlated group whose
    /// single arc is tested against the bound parent token.
    #[test]
    fn a_parent_hop_conjunct_is_a_correlated_group() {
        let guard = block_guard(Pattern::And(vec![
            Pattern::HasTag("Page".to_string()),
            Pattern::Parent(Box::new(col("content_type", "source"))),
        ]));
        let classified = classify_guard(&guard, "op:block.probe").expect("within the bounds");
        assert!(classified.residue.is_empty(), "{:?}", classified.residue);
        let mode = only_mode(&classified);
        assert_eq!(places(mode, ArcOrigin::GuardRefinement), ["block.tags"]);
        assert_eq!(mode.hops.len(), 1);
        let group = &mode.hops[0];
        assert_eq!(group.correlation.hops, vec![Hop::parent()]);
        assert_eq!(group.arcs.len(), 1);
        assert_eq!(group.arcs[0].place.to_string(), "block.content_type");
        assert_eq!(group.arcs[0].binding.as_ref(), Some(&group.binding));
    }

    /// A sibling reach: parent then child, one group, two steps.
    #[test]
    fn a_sibling_reach_composes_two_hops_in_one_group() {
        let guard = block_guard(Pattern::Parent(Box::new(Pattern::Child(Box::new(col(
            "source_language",
            "holon_rule",
        ))))));
        let classified = classify_guard(&guard, "op:block.probe").expect("within the bounds");
        assert!(classified.residue.is_empty(), "{:?}", classified.residue);
        let group = &only_mode(&classified).hops[0];
        assert_eq!(group.correlation.hops, vec![Hop::parent(), Hop::child()]);
    }

    /// Both cells of one hop share a binding, so ONE token must satisfy both.
    /// Splitting them would evaluate two existentials and let two different
    /// entities answer.
    #[test]
    fn a_hops_conjuncts_share_one_binding() {
        let guard = block_guard(Pattern::Child(Box::new(Pattern::And(vec![
            col("content_type", "source"),
            col("source_language", "holon_rule"),
        ]))));
        let classified = classify_guard(&guard, "op:block.probe").expect("within the bounds");
        let group = &only_mode(&classified).hops[0];
        assert_eq!(group.arcs.len(), 2);
        assert!(
            group
                .arcs
                .iter()
                .all(|a| a.binding.as_ref() == Some(&group.binding)),
            "every arc of a hop must name the group's binding"
        );
    }

    /// Disjunction becomes firing modes.
    #[test]
    fn a_disjunction_becomes_one_mode_per_disjunct() {
        let guard = block_guard(Pattern::And(vec![
            col("content_type", "source"),
            Pattern::Or(vec![
                col("source_language", "holon_rule"),
                col("source_language", "action"),
            ]),
        ]));
        let classified = classify_guard(&guard, "op:block.probe").expect("within the bounds");
        assert!(classified.residue.is_empty());
        assert_eq!(classified.modes.len(), 2);
        for mode in &classified.modes {
            assert_eq!(
                places(mode, ArcOrigin::GuardRefinement).len(),
                2,
                "each mode carries the shared conjunct and its own disjunct"
            );
        }
    }

    /// Disjunction lifts OUT of a hop, because an existential distributes
    /// over it.
    #[test]
    fn a_disjunction_inside_a_hop_lifts_to_two_modes() {
        let guard = block_guard(Pattern::Child(Box::new(Pattern::Or(vec![
            col("source_language", "holon_rule"),
            col("source_language", "action"),
        ]))));
        let classified = classify_guard(&guard, "op:block.probe").expect("within the bounds");
        assert!(classified.residue.is_empty(), "{:?}", classified.residue);
        assert_eq!(classified.modes.len(), 2);
        for mode in &classified.modes {
            assert_eq!(mode.hops.len(), 1, "each mode holds its own hop");
            assert_eq!(mode.hops[0].arcs.len(), 1);
        }
    }

    /// Negated existence with a clock builtin stays residue until the
    /// inhibitor lands, and the footprint keeps the reads honest.
    #[test]
    fn a_negated_existence_guard_is_residue_with_a_full_footprint() {
        let guard = Guard::parse("not block_exists(\"Journals/{today}\")").unwrap();
        let classified = classify_guard(&guard, "op:block.probe").expect("within the bounds");
        let mode = only_mode(&classified);
        assert!(places(mode, ArcOrigin::GuardRefinement).is_empty());
        assert_eq!(classified.residue.len(), 1);
        let footprint = places(mode, ArcOrigin::GuardFootprint);
        for expected in [
            "block.id",
            "block.content",
            "block.parent_id",
            "clock.today",
        ] {
            assert!(
                footprint.contains(&expected.to_string()),
                "{expected} missing: {footprint:?}"
            );
        }
    }

    /// A hop whose body the arc language cannot refine stays residue, with
    /// its footprint — the fail-soft that keeps the offer path three-valued.
    #[test]
    fn a_hop_over_an_inexpressible_body_stays_residue() {
        let guard = Guard::parse("parent(not has_tag(\"Page\"))").unwrap();
        let classified = classify_guard(&guard, "op:block.probe").expect("within the bounds");
        assert_eq!(classified.residue.len(), 1);
        let footprint = places(only_mode(&classified), ArcOrigin::GuardFootprint);
        assert!(
            footprint.contains(&"block.parent_id".to_string()),
            "{footprint:?}"
        );
        assert!(
            footprint.contains(&"block.tags".to_string()),
            "{footprint:?}"
        );
    }

    /// A chain longer than the bound is a LOUD error naming the guard, not
    /// silent residue: an author whose predicate stopped being analyzable
    /// gets told why.
    #[test]
    fn a_hop_chain_beyond_the_bound_is_a_named_error() {
        let guard = block_guard(Pattern::Parent(Box::new(Pattern::Parent(Box::new(
            Pattern::Parent(Box::new(col("content_type", "source"))),
        )))));
        let err = classify_guard(&guard, "op:block.move_block").expect_err("over the bound");
        assert!(
            matches!(
                &err,
                NetCompileError::HopChainTooLong { transition, depth, limit }
                    if transition == "op:block.move_block" && *depth == 3 && *limit == MAX_HOP_CHAIN
            ),
            "{err}"
        );
        let text = err.to_string();
        assert!(
            text.contains("op:block.move_block") && text.contains("sibling"),
            "the message must name the guard and the remedy: {text}"
        );
    }

    /// Likewise for the mode bound, and the partial cross-product never
    /// escapes as a weaker predicate.
    #[test]
    fn a_guard_over_the_mode_bound_is_a_named_error() {
        // Four binary disjunctions conjoined: 2^4 = 16 modes, over the 8 bound.
        let disjunction =
            |a: &str, b: &str| Pattern::Or(vec![col("content_type", a), col("content_type", b)]);
        let guard = block_guard(Pattern::And(vec![
            disjunction("a", "b"),
            disjunction("c", "d"),
            disjunction("e", "f"),
            disjunction("g", "h"),
        ]));
        let err = classify_guard(&guard, "rule:block:noisy").expect_err("over the bound");
        assert!(
            matches!(
                &err,
                NetCompileError::TooManyModes { transition, modes, limit }
                    if transition == "rule:block:noisy" && *modes > MAX_MODES && *limit == MAX_MODES
            ),
            "{err}"
        );
    }

    /// `sibling(p)` is ONE hop keyed on `parent_id`, not `parent` then
    /// `child`: the composition needs a parent ROW and so reaches nothing
    /// from a root block, while the profile's own lookup treats two roots as
    /// siblings.
    #[test]
    fn a_sibling_is_one_parent_id_keyed_hop_excluding_the_subject() {
        let guard = block_guard(Pattern::Sibling(Box::new(col(
            "source_language",
            "holon_rule",
        ))));
        let classified = classify_guard(&guard, "op:block.probe").expect("within the bounds");
        assert!(classified.residue.is_empty(), "{:?}", classified.residue);
        let group = &only_mode(&classified).hops[0];
        assert_eq!(group.correlation.hops, vec![Hop::sibling()]);
        assert!(
            group.correlation.exclude_subject,
            "a block is not its own sibling"
        );
        assert_eq!(
            (
                Hop::sibling().from.to_string(),
                Hop::sibling().to.to_string()
            ),
            ("block.parent_id".to_string(), "block.parent_id".to_string()),
            "the sibling hop is keyed on parent_id at both ends"
        );
    }

    /// The `sibling` keyword reaches the AST through the text parser.
    #[test]
    fn the_sibling_keyword_parses() {
        let guard = Guard::parse("sibling(has_tag(\"Page\"))").unwrap();
        assert!(
            matches!(guard.body, Pattern::Sibling(_)),
            "{:?}",
            guard.body
        );
    }
}
