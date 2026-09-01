//! `.cook` file-format adapter — Tier R/O.
//!
//! Rides the same `FileFormatAdapter` seam as org and the markdown flavors.
//! The write half REFUSES loudly: `.cook` files in the vault are authoritative
//! and Inc A ships no renderer, so a write would be loss, not a round trip.

use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;
use cooklang::model::Content;
use holon_api::EntityUri;
use holon_api::StorageEntity;
use holon_api::block::Block;
use holon_core::file_format::FileFormatAdapter;
use holon_core::file_format::FileFormatParseResult;
use holon_core::file_format::WriteTier;
use holon_core::file_format::WritebackDropVerdict;

use crate::cook::STEP_NUMBER_KEY;
use crate::cook::metadata_value_text;
use crate::cook::parse_recipe;
use crate::cook::step_text;

#[derive(Debug, Default, Clone, Copy)]
pub struct CookFormatAdapter;

impl CookFormatAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl FileFormatAdapter for CookFormatAdapter {
    fn extensions(&self) -> &'static [&'static str] {
        &["cook"]
    }

    fn write_tier(&self) -> WriteTier {
        WriteTier::ReadOnly
    }

    fn parse(
        &self,
        path: &Path,
        content: &str,
        parent_dir_id: &EntityUri,
        root: &Path,
    ) -> Result<FileFormatParseResult> {
        let recipe = parse_recipe(content)?;

        // No placeholder title: a name we invented would look like the
        // recipe's own and quietly become its identity.
        let stem = match path.file_stem() {
            Some(stem) => stem
                .to_str()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "recipe path {} has a non-UTF-8 file name and cannot title a recipe",
                        path.display()
                    )
                })?
                .to_string(),
            None => anyhow::bail!(
                "recipe path {} has no file name to title a recipe with",
                path.display()
            ),
        };
        let title = recipe.metadata.title().map(str::to_string).unwrap_or(stem);

        let rel = path.strip_prefix(root).unwrap_or(path);
        let file_id = EntityUri::file(&rel.to_string_lossy());
        let mut document = Block::new_text(file_id.clone(), parent_dir_id.clone(), title);
        document.set_page(true);

        // Every metadata key except the title becomes a document property.
        // Nothing here is skipped quietly: an unrepresentable key or value is
        // refused by name, because a recipe's `tags:` disappearing without a
        // word is the silent-degradation outcome the error ladder forbids.
        for (k, v) in &recipe.metadata.map {
            let key = k.as_str().ok_or_else(|| {
                anyhow::anyhow!(
                    "cooklang metadata key {k:?} is not a string and cannot name a property"
                )
            })?;
            if key.eq_ignore_ascii_case("title") {
                continue;
            }
            if crate::cook::names_block_storage_column(key) {
                anyhow::bail!(
                    "cooklang metadata key {key:?} names a block storage column; storing it \
                     would overwrite the block's own row state. Rename the key."
                );
            }
            document.set_property(key.to_string(), metadata_value_text(key, v)?);
        }

        let mut blocks: Vec<Block> = Vec::new();
        let mut seq = 0usize;
        for section in &recipe.sections {
            for content_item in &section.content {
                let id = EntityUri::block(&format!("{}::b::{}", file_id.id(), seq));
                seq += 1;
                match content_item {
                    Content::Step(step) => {
                        let text = step_text(&recipe, &step.items);
                        let mut b = Block::new_text(id, file_id.clone(), text);
                        b.set_property(STEP_NUMBER_KEY.to_string(), step.number.to_string());
                        blocks.push(b);
                    }
                    Content::Text(text) => {
                        blocks.push(Block::new_text(
                            id,
                            file_id.clone(),
                            text.trim().to_string(),
                        ));
                    }
                }
            }
        }

        Ok(FileFormatParseResult {
            document,
            blocks,
            // Nothing is written back, so no block needs an id minted for
            // re-rendering.
            blocks_needing_ids: Vec::new(),
        })
    }

    fn render_document(&self, _: &Block, _: &[Block], path: &Path, _: &EntityUri) -> String {
        // Unreachability assert, not input handling: this adapter is
        // registered read-only, so no caller has a render path to here.
        // Reaching it means the adapter was wired somewhere that writes.
        unreachable!(
            "CookFormatAdapter is registered read-only; render_document must be unreachable — \
             reaching it for {} is a wiring bug, not bad input. It ships no cooklang renderer, \
             so writing a reconstructed file over an authoritative recipe would be loss.",
            path.display()
        );
    }

    fn render_blocks(&self, _: &[Block], path: &Path, _: &EntityUri) -> String {
        unreachable!(
            "CookFormatAdapter is registered read-only; render_blocks must be unreachable — \
             reaching it for {} is a wiring bug, not bad input.",
            path.display()
        );
    }

    fn doc_id_from_content(&self, _: &str) -> Option<String> {
        // Cooklang embeds no stable id; recipe identity is the vault-relative
        // path, which the caller resolves by name chain.
        None
    }

    fn build_block_params(
        &self,
        block: &Block,
        parent_id: &EntityUri,
        document_uri: &EntityUri,
        previous: Option<&Block>,
    ) -> StorageEntity {
        // The trait returns params, not a Result, so the refusal cannot be
        // propagated here. It is unreachable through this adapter anyway: the
        // parse boundary already refuses a metadata key that names a storage
        // column, so a block reaching here with one was not built by us.
        crate::params::build_block_params(block, parent_id, document_uri, previous).expect(
            "CookFormatAdapter parsed this block, and parse refuses storage-column property keys \
             — a failure here means the block came from elsewhere",
        )
    }

    fn content_differs(&self, a: &Block, b: &Block) -> bool {
        a.content != b.content
    }

    fn sync_document_metadata(&self, _: &Block, _: &mut Block) -> bool {
        false
    }

    fn writeback_drops(
        &self,
        path: &Path,
        _: &str,
        _: &str,
        _: &[(&Path, &str)],
        _: &HashSet<String>,
        _: &Path,
    ) -> Result<WritebackDropVerdict> {
        anyhow::bail!(
            "CookFormatAdapter (Tier R/O) refuses write-back to authoritative recipe file {}",
            path.display()
        )
    }
}
