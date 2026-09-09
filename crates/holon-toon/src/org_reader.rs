//! Minimal Org reader + Org renderers, used only by `examples/measure.rs` to
//! ground the token comparison in real vault files.
//!
//! Scope: exactly the constructs the motivating vault files use — headlines
//! (`*+ [STATE] [#P] title :tags:`), `:PROPERTIES:`/`:END:` drawers with `:ID:`
//! and arbitrary keys, and multi-line bodies. Both `render_org_full` (canonical
//! 3-line ID drawer) and `render_org_compressed` (ID as a trailing headline
//! token, no drawer scaffolding for it) reconstruct from the *same* [`Forest`],
//! so the org-vs-TOON comparison is apples-to-apples.
//!
//! This is a measurement fixture, not a production parser; it is deliberately
//! not part of the round-trip PBT. The edge-typed drawers are the exception:
//! they follow `holon_org_format::parser`'s splitting and sentinel rules (see
//! [`edge_targets`]), because a fixture that reads them differently skews the
//! token measurements it exists to produce.

use std::collections::BTreeMap;

use crate::error::Result;
use crate::error::ToonError;
use crate::models::BlockId;
use crate::models::BlockNode;
use crate::models::Forest;
use crate::models::Priority;
use crate::models::TaskState;
use crate::models::ToonBlock;

const KEYWORDS: &[&str] = &[
    "TODO",
    "DOING",
    "DONE",
    "CANCELLED",
    "CLOSED",
    "LATER",
    "NOW",
];
const DONE_KEYWORDS: &[&str] = &["DONE", "CANCELLED", "CLOSED"];

pub fn is_done(state: &Option<TaskState>) -> bool {
    matches!(state, Some(s) if DONE_KEYWORDS.contains(&s.keyword()))
}

// ---------------------------------------------------------------------------
// Org -> Forest
// ---------------------------------------------------------------------------

struct Building {
    level: u16,
    block: ToonBlock,
    body: Vec<String>,
    props: BTreeMap<String, String>,
    id: Option<String>,
    /// Every place this headline named a priority — the `[#A]` cookie and each
    /// drawer spelling — as `(carrier, priority)`. Collected rather than
    /// overwritten so disagreement between ANY two is caught, which is the rule
    /// `holon_org_format::parser` applies.
    priority_carriers: Vec<(String, Priority)>,
}

pub fn parse_org(input: &str) -> Result<Forest> {
    let mut flat: Vec<Building> = Vec::new();
    let mut in_drawer = false;

    for line in input.lines() {
        if let Some((level, rest)) = headline_prefix(line) {
            in_drawer = false;
            let (state, priority, title, tags) = parse_headline_rest(rest)?;
            let mut block = ToonBlock::text(BlockId::new("PENDING").unwrap(), title);
            block.state = state;
            block.tags = tags;
            flat.push(Building {
                level,
                block,
                body: Vec::new(),
                props: BTreeMap::new(),
                id: None,
                priority_carriers: priority
                    .map(|p| (format!("the cookie `[#{}]`", p.letter()), p))
                    .into_iter()
                    .collect(),
            });
            continue;
        }
        if line.trim() == ":PROPERTIES:" {
            in_drawer = true;
            continue;
        }
        if line.trim() == ":END:" {
            in_drawer = false;
            continue;
        }
        if in_drawer {
            if let Some((k, v)) = parse_drawer_line(line) {
                if let Some(cur) = flat.last_mut() {
                    let owner = cur.id.clone().unwrap_or_else(|| cur.block.title.clone());
                    if k.eq_ignore_ascii_case("ID") {
                        cur.id = Some(v);
                    } else if k.eq_ignore_ascii_case(PRIORITY) {
                        // `:priority: A` is the DRAWER spelling of the cookie,
                        // case-insensitive as org drawer keys are. It resolves
                        // into the typed field here; leaving it in `props`
                        // would make this reader answer "no priority" for the
                        // spelling the vault actually uses.
                        let p = single_letter(&v)
                            .and_then(Priority::from_letter)
                            .ok_or_else(|| ToonError::BadOrgPriority {
                                headline: cur.block.title.clone(),
                                cookie: v.clone(),
                            })?;
                        cur.priority_carriers.push((format!("`:{k}: {v}`"), p));
                    } else if is_dependency_key(&k) {
                        cur.block.requires = edge_targets(&v, &k, &owner)?;
                    } else if k.eq_ignore_ascii_case(CONTRIBUTES_TO) {
                        cur.block.contributes_to = edge_targets(&v, &k, &owner)?;
                    } else {
                        cur.props.insert(k, v);
                    }
                }
            }
            continue;
        }
        if line.starts_with("#+") {
            continue; // file-level directive
        }
        // Body line for the current block (if any).
        if let Some(cur) = flat.last_mut() {
            cur.body.push(line.to_string());
        }
    }

    // Finalize bodies/props/ids. A headline with no :ID: drawer gets a
    // disclosed synthetic id (`no-id-<n>`) so the measurement fixture stays
    // total; real vault headlines all carry an :ID:.
    let mut finalized: Vec<(u16, ToonBlock)> = Vec::with_capacity(flat.len());
    for (i, mut b) in flat.into_iter().enumerate() {
        // ALLOW(fallback): disclosed synthetic id for a drawer-less headline in
        // the measurement fixture (prefixed `no-id-`); not a silent default.
        let id = b.id.take().unwrap_or_else(|| format!("no-id-{}", i));
        b.block.id = BlockId::new(id).expect("synthetic/real ids are whitespace-free");
        b.block.priority = reconcile_priority(&b.priority_carriers, &b.block.title)?;
        b.block.properties = b.props;
        // Trim trailing blank body lines; empty body -> None.
        while b.body.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
            b.body.pop();
        }
        b.block.body = if b.body.is_empty() {
            None
        } else {
            Some(b.body.join("\n"))
        };
        finalized.push((b.level.saturating_sub(1), b.block));
    }

    Ok(build_forest(finalized))
}

/// The drawer spelling of the priority cookie. Case-insensitive, like every
/// org drawer key.
const PRIORITY: &str = "priority";

/// `Some(c)` when `s` is exactly one character, so a multi-character drawer
/// value is refused rather than read from its first byte.
fn single_letter(s: &str) -> Option<char> {
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

/// Every carrier must name the same priority. Two that disagree is an authoring
/// mistake with no correct answer, so it refuses the parse rather than letting
/// scan order decide what the file means.
fn reconcile_priority(carriers: &[(String, Priority)], headline: &str) -> Result<Option<Priority>> {
    let Some((first_name, first)) = carriers.first() else {
        return Ok(None);
    };
    for (name, p) in &carriers[1..] {
        if p != first {
            return Err(ToonError::DisagreeingOrgPriority {
                headline: headline.to_string(),
                first: first_name.clone(),
                second: name.clone(),
            });
        }
    }
    Ok(Some(*first))
}

/// Number of leading `*` followed by a space, plus the remainder.
fn headline_prefix(line: &str) -> Option<(u16, &str)> {
    let stars = line.chars().take_while(|&c| c == '*').count();
    if stars == 0 {
        return None;
    }
    let rest = &line[stars..];
    let rest = rest.strip_prefix(' ')?;
    Some((stars as u16, rest))
}

type HeadlineRest = (Option<TaskState>, Option<Priority>, String, Vec<String>);

fn parse_headline_rest(rest: &str) -> Result<HeadlineRest> {
    let mut s = rest.trim_end();

    // State keyword.
    let mut state = None;
    for kw in KEYWORDS {
        if let Some(after) = s.strip_prefix(kw) {
            if after.is_empty() || after.starts_with(' ') {
                state = TaskState::new(*kw);
                s = after.trim_start();
                break;
            }
        }
    }

    // Priority [#A]. A one-character cookie that is not a letter has no
    // priority at all and is refused loudly — the verdict
    // `holon_org_format::parser` reaches, so the two readers cannot disagree
    // about what a file means. Scanning to the closing bracket rather than
    // indexing byte 1 is what lets a multi-byte `[#Ä]` reach that refusal
    // instead of passing silently as title text.
    let mut priority = None;
    if let Some(after) = s.strip_prefix("[#") {
        if let Some(close) = after.find(']') {
            let mut inner = after[..close].chars();
            if let (Some(c), None) = (inner.next(), inner.next()) {
                priority =
                    Some(
                        Priority::from_letter(c).ok_or_else(|| ToonError::BadOrgPriority {
                            headline: rest.to_string(),
                            cookie: c.to_string(),
                        })?,
                    );
                s = after[close + 1..].trim_start();
            }
        }
    }

    // Trailing tags :a:b:.
    let mut tags = Vec::new();
    if let Some(last) = s.rsplit(' ').next() {
        if is_tag_token(last) {
            tags = last
                .trim_matches(':')
                .split(':')
                .map(String::from)
                .collect();
            s = s[..s.len() - last.len()].trim_end();
        }
    }

    Ok((state, priority, s.to_string(), tags))
}

fn is_tag_token(tok: &str) -> bool {
    tok.len() >= 2
        && tok.starts_with(':')
        && tok.ends_with(':')
        && tok[1..tok.len() - 1]
            .split(':')
            .all(|t| !t.is_empty() && t.chars().all(|c| c.is_alphanumeric() || "_@-".contains(c)))
}

fn parse_drawer_line(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    let t = t.strip_prefix(':')?;
    let colon = t.find(':')?;
    let key = t[..colon].to_string();
    let value = t[colon + 1..].trim().to_string();
    Some((key, value))
}

/// The `:contributes-to:` drawer key (`holon_org_format`'s spelling).
const CONTRIBUTES_TO: &str = "contributes-to";

/// The two org-drawer spellings of the dependency edge. Both name the same
/// field, mirroring `holon_org_format::parser::is_dependency_key`.
fn is_dependency_key(key: &str) -> bool {
    key.eq_ignore_ascii_case("REQUIRES") || key.eq_ignore_ascii_case("BLOCKED-BY")
}

/// Parse an edge-typed drawer value into its targets, mirroring
/// `holon_org_format::parser::parse_edge_targets`: slugs separate on commas or
/// any whitespace, and a slug that names no block is an error rather than a
/// manufactured id.
fn edge_targets(value: &str, key: &str, owner: &str) -> Result<Vec<BlockId>> {
    value
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        // `none` is the authored empty-set sentinel for `:contributes-to:`
        // ONLY: `:REQUIRES: none` has always meant an edge to a block called
        // `none`.
        .filter(|slug| {
            !(key.eq_ignore_ascii_case(CONTRIBUTES_TO) && slug.eq_ignore_ascii_case("none"))
        })
        .map(|slug| {
            BlockId::new(slug)
                .filter(|id| is_bare_slug(id.as_str()))
                .ok_or_else(|| ToonError::BadEdgeSlug {
                    owner: owner.to_string(),
                    key: key.to_string(),
                    slug: slug.to_string(),
                })
        })
        .collect()
}

/// Whether `slug` is a bare id rather than an org link (`[[…]]`) or an
/// unsubstituted template slot (`{{…}}`). This fixture models no templates, so
/// both forms are errors here, as they are in the real parser outside a
/// template subtree.
///
/// Approximate: the real parser runs each slug through `EntityUri`'s RFC 3986
/// charset and so refuses more than these four characters (`a"b`, for one). A
/// file carrying such a slug is refused at ingest and therefore never stored,
/// so only files that could never be measured against a real vault diverge.
fn is_bare_slug(slug: &str) -> bool {
    !slug.contains(['[', ']', '{', '}'])
}

/// Build a nested forest from a `(depth, block)` pre-order list, tolerating
/// skipped levels (parent = nearest shallower ancestor).
fn build_forest(flat: Vec<(u16, ToonBlock)>) -> Forest {
    struct Frame {
        depth: u16,
        block: ToonBlock,
        children: Vec<BlockNode>,
    }
    let mut roots: Vec<BlockNode> = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();

    let close = |stack: &mut Vec<Frame>, roots: &mut Vec<BlockNode>| {
        let f = stack.pop().unwrap();
        let node = BlockNode::with_children(f.block, f.children);
        match stack.last_mut() {
            Some(p) => p.children.push(node),
            None => roots.push(node),
        }
    };

    for (depth, block) in flat {
        while stack.last().map(|f| f.depth >= depth).unwrap_or(false) {
            close(&mut stack, &mut roots);
        }
        stack.push(Frame {
            depth,
            block,
            children: Vec::new(),
        });
    }
    while !stack.is_empty() {
        close(&mut stack, &mut roots);
    }
    Forest::new(roots)
}

// ---------------------------------------------------------------------------
// Filtering
// ---------------------------------------------------------------------------

/// Drop every DONE-state block together with its subtree (the "exclude DONE
/// tasks" projection an agent asks for).
pub fn filter_exclude_done(forest: &Forest) -> Forest {
    fn keep(nodes: &[BlockNode]) -> Vec<BlockNode> {
        nodes
            .iter()
            .filter(|n| !is_done(&n.block.state))
            .map(|n| BlockNode::with_children(n.block.clone(), keep(&n.children)))
            .collect()
    }
    Forest::new(keep(&forest.roots))
}

// ---------------------------------------------------------------------------
// Forest -> Org (two variants)
// ---------------------------------------------------------------------------

/// Canonical org: full `:PROPERTIES:`/`:ID:`/`:END:` drawer per block.
pub fn render_org_full(forest: &Forest) -> String {
    let mut out = String::new();
    render_org_nodes(&forest.roots, 1, &mut out, false);
    out
}

/// Compressed org: `:ID:` inlined as a trailing headline token; the 3-line
/// drawer scaffolding is emitted only when a block carries *other* drawer keys.
pub fn render_org_compressed(forest: &Forest) -> String {
    let mut out = String::new();
    render_org_nodes(&forest.roots, 1, &mut out, true);
    out
}

fn render_org_nodes(nodes: &[BlockNode], level: u16, out: &mut String, compress_id: bool) {
    for node in nodes {
        render_org_block(&node.block, level, out, compress_id);
        render_org_nodes(&node.children, level + 1, out, compress_id);
    }
}

fn render_org_block(block: &ToonBlock, level: u16, out: &mut String, compress_id: bool) {
    out.push_str(&"*".repeat(level as usize));
    out.push(' ');
    if let Some(s) = &block.state {
        out.push_str(s.keyword());
        out.push(' ');
    }
    if let Some(p) = block.priority {
        out.push_str(&format!("[#{}] ", p.letter()));
    }
    out.push_str(&block.title);
    if !block.tags.is_empty() {
        out.push_str(&format!(" :{}:", block.tags.join(":")));
    }
    if compress_id {
        out.push_str(&format!("  {{#{}}}", block.id));
    }
    out.push('\n');

    let has_other_props = !block.properties.is_empty();
    if compress_id {
        if has_other_props {
            out.push_str(":PROPERTIES:\n");
            for (k, v) in &block.properties {
                out.push_str(&format!(":{}: {}\n", k, v));
            }
            out.push_str(":END:\n");
        }
    } else {
        out.push_str(":PROPERTIES:\n");
        out.push_str(&format!(":ID: {}\n", block.id));
        for (k, v) in &block.properties {
            out.push_str(&format!(":{}: {}\n", k, v));
        }
        out.push_str(":END:\n");
    }

    if let Some(body) = &block.body {
        out.push_str(body);
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only_block(input: &str) -> ToonBlock {
        let forest = parse_org(input).expect("fixture parses");
        assert_eq!(forest.roots.len(), 1, "fixture must hold one headline");
        forest.roots[0].block.clone()
    }

    /// This reader carried its own A/B/C-only priority and simply left `[#D]`
    /// sitting in the title — two org readers in one tree disagreeing about
    /// what a file means. Org's range is configurable, so every letter is a
    /// priority here exactly as it is in `holon_org_format::parser`.
    #[test]
    fn a_priority_letter_beyond_the_default_range_is_a_priority_here_too() {
        let block = only_block("* TODO [#D] Ship it\n");
        assert_eq!(
            block.priority,
            Priority::from_letter('D'),
            "`[#D]` is a letter priority, not title text"
        );
        assert_eq!(block.title, "Ship it", "the cookie must leave the title");
    }

    /// A cookie that is not a letter has no priority at all. It is refused
    /// loudly — the org parser's verdict, never a silent drop.
    #[test]
    fn a_non_letter_priority_cookie_is_refused_loudly() {
        let err = parse_org("* TODO [#1] Ship it\n")
            .expect_err("a non-letter priority cookie must refuse the parse");
        assert!(
            matches!(err, ToonError::BadOrgPriority { .. }),
            "expected a priority refusal, got {err:?}"
        );
    }

    fn ids(ids: &[BlockId]) -> Vec<&str> {
        ids.iter().map(BlockId::as_str).collect()
    }

    #[test]
    fn contributes_to_none_is_a_per_slug_sentinel() {
        let b = only_block("* Goal\n:PROPERTIES:\n:ID: p0\n:contributes-to: none goal-1\n:END:\n");
        assert_eq!(ids(&b.contributes_to), vec!["goal-1"]);
    }

    #[test]
    fn edge_slugs_split_on_any_whitespace() {
        let b = only_block("* Goal\n:PROPERTIES:\n:ID: p0\n:REQUIRES:\tdep-1\tdep-2\n:END:\n");
        assert_eq!(ids(&b.requires), vec!["dep-1", "dep-2"]);
    }

    #[test]
    fn the_link_form_is_refused() {
        let err =
            parse_org("* Goal\n:PROPERTIES:\n:ID: p0\n:contributes-to: [[Some Page]]\n:END:\n")
                .expect_err("a link is not a bare block id");
        assert_eq!(
            err,
            ToonError::BadEdgeSlug {
                owner: "p0".into(),
                key: CONTRIBUTES_TO.into(),
                slug: "[[Some".into(),
            }
        );
    }

    #[test]
    fn dependency_keys_are_case_insensitive() {
        let b = only_block("* Goal\n:PROPERTIES:\n:ID: p0\n:requires: dep-1\n:END:\n");
        assert_eq!(ids(&b.requires), vec!["dep-1"]);
        assert!(!b.properties.contains_key("requires"));
    }
}
