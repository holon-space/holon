//! Dense org projection: the agent-facing round-trip representation.
//!
//! A dense projection is ordinary org text with ONE difference from the
//! canonical form: a headline's `:PROPERTIES:/:ID:/:END:` scaffolding is
//! compressed to a single trailing token `{#<alias>}` on the headline line,
//! where `<alias>` is a short per-query handle standing in for the block's bare
//! UUID. Bare UUIDs are ~45% of the tokens in a canonical projection, so the
//! alias is the biggest token lever (ruling: Martin, 2026-07-23 — org
//! container, NOT TOON).
//!
//! This is **projection-only**: on-disk org files keep the official drawer
//! syntax unchanged. The alias table is assigned per query and is meaningless
//! outside the projection/patch cycle it was minted for.
//!
//! There is exactly ONE org syntax implementation: [`render_dense`] shares the
//! canonical renderer's tree walk and headline builder (only the identity
//! emission point differs), and [`parse_dense`] reuses
//! [`crate::parse_org_file`] and strips the trailing token at the boundary — it
//! does not re-parse org.

use std::collections::HashMap;
use std::collections::HashSet;

use anyhow::Result;
use anyhow::bail;
use holon_api::EntityUri;
use holon_api::block::Block;

use crate::models::HeadlineIdentity;
use crate::models::render_document_header;
use crate::models::render_headline_block;
use crate::org_renderer::OrgRenderer;

/// Base62 alphabet for alias encoding. Delimited by `{#…}`, so any length is
/// unambiguous; shortest-fit keeps the token cheap.
const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// A short per-query handle standing in for a block's bare UUID in a dense
/// projection. Base62, projection-only, never persisted.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Alias(String);

impl Alias {
    /// Parse a handle from projection text. Rejects anything that is not a
    /// non-empty base62 string — fail loud at the boundary (Parse, Don't
    /// Validate) rather than silently accept a malformed token.
    pub fn parse(raw: &str) -> Result<Alias> {
        if raw.is_empty() {
            bail!("dense alias is empty");
        }
        if !raw.bytes().all(|b| BASE62.contains(&b)) {
            bail!("dense alias {raw:?} contains non-base62 characters");
        }
        Ok(Alias(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Alias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Encode a zero-based index as a minimal base62 string (`0`, `1`, …, `z`,
/// `10`, …). Deterministic and collision-free per index.
fn encode_base62(mut n: usize) -> String {
    let mut out = Vec::new();
    loop {
        out.push(BASE62[n % 62]);
        n /= 62;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).expect("base62 alphabet is ASCII")
}

/// Bidirectional block-id ⇄ alias table for ONE projection/patch cycle.
///
/// Aliases are assigned deterministically by projection order (first block →
/// `0`, …), so the same block set always yields the same handles. The table is
/// captured at projection time and re-supplied to the patch step to resolve
/// edited handles back to block ids.
#[derive(Debug, Clone, Default)]
pub struct AliasTable {
    to_id: HashMap<Alias, EntityUri>,
    to_alias: HashMap<String, Alias>,
}

impl AliasTable {
    /// Assign minimal base62 aliases to `ids` in iteration order. Duplicate ids
    /// reuse their first alias.
    pub fn assign<I: IntoIterator<Item = EntityUri>>(ids: I) -> AliasTable {
        let mut table = AliasTable::default();
        for id in ids {
            if table.to_alias.contains_key(id.as_str()) {
                continue;
            }
            let alias = Alias(encode_base62(table.to_alias.len()));
            table.to_id.insert(alias.clone(), id.clone());
            table.to_alias.insert(id.as_str().to_string(), alias);
        }
        table
    }

    /// Reconstruct a table from explicit (alias, id) pairs (e.g. rehydrating a
    /// persisted projection handle). Fails loud on a duplicate alias.
    pub fn from_pairs<I: IntoIterator<Item = (Alias, EntityUri)>>(pairs: I) -> Result<AliasTable> {
        let mut table = AliasTable::default();
        for (alias, id) in pairs {
            if table.to_id.contains_key(&alias) {
                bail!("duplicate alias {alias} while rebuilding alias table");
            }
            table
                .to_alias
                .insert(id.as_str().to_string(), alias.clone());
            table.to_id.insert(alias, id);
        }
        Ok(table)
    }

    pub fn alias_of(&self, id: &EntityUri) -> Option<&Alias> {
        self.to_alias.get(id.as_str())
    }

    pub fn id_of(&self, alias: &Alias) -> Option<&EntityUri> {
        self.to_id.get(alias)
    }

    pub fn len(&self) -> usize {
        self.to_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.to_id.is_empty()
    }

    /// Iterate (alias, id) pairs — for persisting a projection handle.
    pub fn pairs(&self) -> impl Iterator<Item = (&Alias, &EntityUri)> {
        self.to_id.iter()
    }
}

/// Render a single headline block in dense token form. Source/Image blocks keep
/// their canonical form (the token compression targets headline `:ID:`
/// drawers). Fails loud if the block is missing from `alias_table` — a
/// projected block MUST have an alias.
pub(crate) fn to_org_dense(
    block: &Block,
    alias_table: &AliasTable,
    gap_ids: &HashSet<String>,
) -> String {
    use holon_api::types::ContentType;
    if matches!(block.content_type, ContentType::Source | ContentType::Image) {
        return crate::models::ToOrg::to_org(block);
    }
    let alias = alias_table.alias_of(&block.id).unwrap_or_else(|| {
        panic!(
            "dense render: block {} has no alias in the projection table — every projected block \
             must be aliased",
            block.id.as_str()
        )
    });
    let gap = gap_ids.contains(block.id.as_str());
    render_headline_block(
        block,
        HeadlineIdentity::DenseToken {
            alias: alias.as_str(),
            gap,
        },
    )
}

/// Render a projected block forest as a dense org document: header
/// (`#+TITLE`/`#+TODO`) + headlines, with `:ID:` drawers compressed to trailing
/// `{#alias}` tokens.
///
/// `blocks` arrive in authoritative tree order (parent before children),
/// exactly as [`OrgRenderer::render_entitys`] expects. `alias_table` must cover
/// every headline block in `blocks`. `gap_ids` are block ids to flag with the
/// elided-ancestor marker (`{#alias^}`) — blocks rendered under a parent that
/// is not their true parent.
pub fn render_dense(
    doc_block: &Block,
    blocks: &[Block],
    file_id: &EntityUri,
    alias_table: &AliasTable,
    gap_ids: &HashSet<String>,
) -> String {
    let mut result = render_document_header(doc_block);
    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }
    result.push_str(&OrgRenderer::render_entitys_dense(
        blocks,
        file_id,
        alias_table,
        gap_ids,
    ));
    while matches!(
        result.chars().last(),
        Some('\n') | Some(' ') | Some('\t') | Some('\r')
    ) {
        result.pop();
    }
    result.push('\n');
    result
}

/// One block in a parsed dense projection.
#[derive(Debug, Clone)]
pub struct DenseBlock {
    /// The parsed block, with the trailing `{#alias}` token stripped from its
    /// title. Its `id`/`parent_id` are parser-minted placeholders (dense text
    /// carries no real ids) — identity comes from [`Self::alias`].
    pub block: Block,
    /// The `{#alias}` handle, or `None` for a NEW block (no token present).
    pub alias: Option<Alias>,
    /// Whether the token carried the elided-ancestor gap marker (`{#alias^}`).
    /// Display-only: a patch treats it as noise — its presence/absence carries
    /// no semantic weight (moves are detected by relative position, not this).
    pub gap: bool,
    /// Parser-minted placeholder id for THIS row — used to reconstruct the
    /// parent→child tree among parsed rows (including NEW blocks).
    pub parse_id: EntityUri,
    /// Placeholder id of the parent row, or `None` when this row's parent is
    /// the projection anchor (a projection root), not another parsed row.
    pub parent_parse_id: Option<EntityUri>,
}

/// Result of parsing a dense projection back into typed blocks.
#[derive(Debug, Clone)]
pub struct DenseParse {
    /// Blocks in document order.
    pub blocks: Vec<DenseBlock>,
}

/// Split a trailing `{#<alias>}` or `{#<alias>^}` token off a headline title's
/// first line. Returns `(clean_title_first_line, Some((alias, gap)))` when
/// present, else `(unchanged, None)`. `gap` is the trailing `^` elided-ancestor
/// marker. Only a token that is the very last non-space run and matches the
/// grammar is treated as a token.
fn split_trailing_token(first_line: &str) -> Result<(String, Option<(Alias, bool)>)> {
    let trimmed = first_line.trim_end();
    let Some(open) = trimmed.rfind("{#") else {
        return Ok((first_line.to_string(), None));
    };
    if !trimmed.ends_with('}') {
        return Ok((first_line.to_string(), None));
    }
    let mut inner = &trimmed[open + 2..trimmed.len() - 1];
    let gap = inner.ends_with('^');
    if gap {
        inner = &inner[..inner.len() - 1];
    }
    // Guard against a false positive where `{#…}` is not a clean token (empty or
    // a non-base62 char) — leave the title untouched.
    if inner.is_empty() || !inner.bytes().all(|b| BASE62.contains(&b)) {
        return Ok((first_line.to_string(), None));
    }
    let alias = Alias::parse(inner)?;
    let clean = trimmed[..open].trim_end().to_string();
    Ok((clean, Some((alias, gap))))
}

/// Parse a dense projection back into typed blocks, reusing the canonical org
/// parser. The trailing `{#alias}` token is stripped from each headline at the
/// boundary and recorded as [`DenseBlock::alias`]. Fails loud on malformed
/// input (no `.ok()` swallowing).
pub fn parse_dense(text: &str) -> Result<DenseParse> {
    let path = std::path::Path::new("dense_projection.org");
    let root = std::path::Path::new("");
    let parent_dir_id = EntityUri::block("dense-projection-anchor");
    let parsed = crate::parse_org_file(path, text, &parent_dir_id, root)?;
    let doc_id = parsed.document.id.clone();

    let mut blocks = Vec::with_capacity(parsed.blocks.len());
    for mut block in parsed.blocks {
        let parse_id = block.id.clone();
        let parent_parse_id = if block.parent_id == doc_id {
            None
        } else {
            Some(block.parent_id.clone())
        };

        // Strip the trailing token from the title's first line, preserving any
        // body lines. Source/Image blocks never carry a token.
        let (alias, gap) = {
            let content = block.content.clone();
            let mut lines = content.lines();
            let first = lines.next().unwrap_or("");
            let (clean_first, token) = split_trailing_token(first)?;
            if token.is_some() {
                let rest: Vec<&str> = lines.collect();
                block.content = if rest.is_empty() {
                    clean_first
                } else {
                    format!("{}\n{}", clean_first, rest.join("\n"))
                };
            }
            match token {
                Some((alias, gap)) => (Some(alias), gap),
                None => (None, false),
            }
        };

        blocks.push(DenseBlock {
            block,
            alias,
            gap,
            parse_id,
            parent_parse_id,
        });
    }

    Ok(DenseParse { blocks })
}
