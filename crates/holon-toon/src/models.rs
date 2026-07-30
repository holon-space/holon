//! Domain model for the TOON block projection.
//!
//! This is a faithful, self-contained projection of the fields a Holon `Block`
//! carries through the org boundary (`crates/holon-org-format`), reduced to the
//! set an agent reads and patches: identity, task metadata, inline-verbatim
//! content, arbitrary drawer properties, and the structural edges (parent via
//! nesting, `requires`). Field names deliberately mirror `holon_api::Block` so
//! that, if the experiment is adopted, promotion is a mechanical move rather
//! than a re-model.
//!
//! Illegal states are unrepresentable where cheap: the forest is a genuine tree
//! ([`BlockNode`] owns its `children`), so depth is *implied by nesting* and
//! can never contradict a stored level. [`BlockId`] rejects whitespace at
//! construction. Content-type is an enum, not a stringly value.

use std::collections::BTreeMap;

/// A bare block identifier (no `block:` scheme — org files store bare IDs, see
/// `docs/Reference/ORG_SYNTAX.md`). Guaranteed non-empty and whitespace-free so
/// it can occupy an unquoted TOON cell / a space-separated `REQUIRES` list.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockId(String);

impl BlockId {
    /// Parse at the boundary. Returns `None` for empty or whitespace-bearing
    /// input so the caller can fail loud with row context.
    pub fn new(s: impl Into<String>) -> Option<Self> {
        let s = s.into();
        if s.is_empty() || s.chars().any(char::is_whitespace) {
            return None;
        }
        Some(BlockId(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BlockId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Block content kind, mirroring `holon_api::types::ContentType` (the subset
/// that survives the org boundary). `Source` carries a language; `Image`
/// carries a bare file path in `content`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentType {
    Text,
    Source,
    Image,
}

/// Org headline priority (`[#A]`/`[#B]`/`[#C]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    A,
    B,
    C,
}

impl Priority {
    pub fn letter(self) -> char {
        match self {
            Priority::A => 'A',
            Priority::B => 'B',
            Priority::C => 'C',
        }
    }

    pub fn from_letter(c: char) -> Option<Self> {
        match c {
            'A' => Some(Priority::A),
            'B' => Some(Priority::B),
            'C' => Some(Priority::C),
            _ => None,
        }
    }
}

/// A TODO keyword such as `TODO`, `DOING`, `DONE`. Kept as a validated newtype
/// rather than a `String`: it must be a single uppercase-ish word (no
/// whitespace) so it can occupy the unquoted `state` column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskState(String);

impl TaskState {
    pub fn new(keyword: impl Into<String>) -> Option<Self> {
        let s = keyword.into();
        if s.is_empty() || s.chars().any(char::is_whitespace) {
            return None;
        }
        Some(TaskState(s))
    }

    pub fn keyword(&self) -> &str {
        &self.0
    }
}

/// One block, sans structural position (position is owned by [`BlockNode`]).
///
/// `title` is the headline text and `body` the remaining lines — both stored
/// **inline-verbatim**: `[[links]]`, `*bold*`, `/italic/` etc. are opaque text
/// the TOON layer never touches (per the experiment brief, TOON replaces only
/// the *structural* layer). For a `Source` block, `body` holds the code and
/// `source_language` the fence language; `title` is empty. For an `Image`
/// block, `content_path` holds the bare file path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToonBlock {
    pub id: BlockId,
    pub state: Option<TaskState>,
    pub priority: Option<Priority>,
    pub tags: Vec<String>,
    pub content_type: ContentType,
    pub source_language: Option<String>,
    pub source_name: Option<String>,
    /// Headline text (Text blocks) — inline-verbatim. Empty for Source/Image.
    pub title: String,
    /// Body lines after the headline (Text) or the code (Source). `None` = no
    /// body. Image blocks put the bare path here as `content_path` instead.
    pub body: Option<String>,
    /// Bare file path for `Image` blocks (`content` in the org model).
    pub content_path: Option<String>,
    pub scheduled: Option<String>,
    pub deadline: Option<String>,
    /// Dependency edge — bare block ids this block requires / is blocked by.
    pub requires: Vec<BlockId>,
    pub advice_suppressed: Vec<BlockId>,
    pub collapsed: bool,
    pub widget_only: bool,
    /// Arbitrary `:PROPERTIES:` drawer keys other than `:ID:` and the fields
    /// promoted above (assigned-to, claimed-at, …). Ordered for determinism.
    pub properties: BTreeMap<String, String>,
}

impl ToonBlock {
    /// Minimal Text block: an id and a title, everything else empty.
    pub fn text(id: BlockId, title: impl Into<String>) -> Self {
        ToonBlock {
            id,
            state: None,
            priority: None,
            tags: Vec::new(),
            content_type: ContentType::Text,
            source_language: None,
            source_name: None,
            title: title.into(),
            body: None,
            content_path: None,
            scheduled: None,
            deadline: None,
            requires: Vec::new(),
            advice_suppressed: Vec::new(),
            collapsed: false,
            widget_only: false,
            properties: BTreeMap::new(),
        }
    }
}

/// A node in the block forest — a block plus its ordered children. The tree
/// shape *is* the parent/child structure; there is no separate parent pointer
/// to fall out of sync.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockNode {
    pub block: ToonBlock,
    pub children: Vec<BlockNode>,
}

impl BlockNode {
    pub fn leaf(block: ToonBlock) -> Self {
        BlockNode {
            block,
            children: Vec::new(),
        }
    }

    pub fn with_children(block: ToonBlock, children: Vec<BlockNode>) -> Self {
        BlockNode { block, children }
    }
}

/// A forest of top-level blocks (an org file / a filtered projection is a
/// forest, not a single tree).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Forest {
    pub roots: Vec<BlockNode>,
}

impl Forest {
    pub fn new(roots: Vec<BlockNode>) -> Self {
        Forest { roots }
    }

    /// Depth-first pre-order flatten, pairing each block with its 0-based
    /// depth. This is exactly the row order the TOON table serializes.
    pub fn flatten(&self) -> Vec<(u16, &ToonBlock)> {
        let mut out = Vec::new();
        fn walk<'a>(nodes: &'a [BlockNode], depth: u16, out: &mut Vec<(u16, &'a ToonBlock)>) {
            for node in nodes {
                out.push((depth, &node.block));
                walk(&node.children, depth + 1, out);
            }
        }
        walk(&self.roots, 0, &mut out);
        out
    }

    pub fn block_count(&self) -> usize {
        self.flatten().len()
    }
}
