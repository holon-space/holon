//! The shipped `Journals.org` asset as the reference models it: the same
//! parse the seed runs, so the oracle and the SUT never disagree on its ids.

use holon_api::ContentType;
use holon_api::EntityUri;
use holon_api::block::Block;

pub fn blocks() -> Vec<Block> {
    holon_frontend::DEFAULT_ASSETS
        .iter()
        .find(|asset| asset.filename == "Journals.org")
        .expect("Journals.org is a default asset")
        .blocks()
        .expect("parse the shipped Journals.org asset")
}

pub fn page_id() -> EntityUri {
    blocks().swap_remove(0).id
}

fn is_display(block: &Block, page: &EntityUri) -> bool {
    block.parent_id == *page && block.content_type == ContentType::Source
}

/// The page's own query and render sources.
pub fn display_ids() -> Vec<EntityUri> {
    let page = page_id();
    blocks()
        .into_iter()
        .filter(|b| is_display(b, &page))
        .map(|b| b.id)
        .collect()
}

/// The auto-create rule: its heading and every block under it.
pub fn rule_ids() -> Vec<EntityUri> {
    let page = page_id();
    blocks()
        .into_iter()
        .filter(|b| b.id != page && !is_display(b, &page))
        .map(|b| b.id)
        .collect()
}
