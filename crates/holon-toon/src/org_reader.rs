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
//! not part of the round-trip PBT.

use std::collections::BTreeMap;

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
}

pub fn parse_org(input: &str) -> Forest {
    let mut flat: Vec<Building> = Vec::new();
    let mut in_drawer = false;

    for line in input.lines() {
        if let Some((level, rest)) = headline_prefix(line) {
            in_drawer = false;
            let (state, priority, title, tags) = parse_headline_rest(rest);
            let mut block = ToonBlock::text(BlockId::new("PENDING").unwrap(), title);
            block.state = state;
            block.priority = priority;
            block.tags = tags;
            flat.push(Building {
                level,
                block,
                body: Vec::new(),
                props: BTreeMap::new(),
                id: None,
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
                    match k.as_str() {
                        "ID" => cur.id = Some(v),
                        // Typed edge field (both accepted spellings), mirroring
                        // the real org parser: bare ids, whitespace/comma
                        // separated. Routes to `requires`, not the drawer map.
                        "REQUIRES" | "BLOCKED-BY" => {
                            cur.block.requires = v
                                .split([' ', ','])
                                .filter(|s| !s.is_empty())
                                .filter_map(BlockId::new)
                                .collect();
                        }
                        _ => {
                            cur.props.insert(k, v);
                        }
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

    build_forest(finalized)
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

fn parse_headline_rest(rest: &str) -> (Option<TaskState>, Option<Priority>, String, Vec<String>) {
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

    // Priority [#A].
    let mut priority = None;
    if let Some(after) = s.strip_prefix("[#") {
        if after.len() >= 2 && after.as_bytes()[1] == b']' {
            if let Some(p) = Priority::from_letter(after.as_bytes()[0] as char) {
                priority = Some(p);
                s = after[2..].trim_start();
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

    (state, priority, s.to_string(), tags)
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
