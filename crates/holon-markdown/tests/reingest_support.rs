use std::path::Path;

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_core::file_format::FileFormatAdapter;

/// The `milk` block of `source` parsed as `Groceries.md`, and its document id.
pub fn parse_one(adapter: &dyn FileFormatAdapter, source: &str) -> (Block, EntityUri) {
    let root = Path::new("/vault");
    let parsed = adapter
        .parse(
            &root.join("Groceries.md"),
            source,
            &EntityUri::no_parent(),
            root,
        )
        .unwrap();
    let block = parsed
        .blocks
        .iter()
        .find(|b| b.content.contains("milk"))
        .unwrap_or_else(|| panic!("no milk block in {:?}", parsed.blocks))
        .clone();
    (block, parsed.document.id.clone())
}
