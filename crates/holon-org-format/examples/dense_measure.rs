//! Token/size measurement: canonical org (what `read_org_file` returns) vs the
//! DENSE projection, on a real-SHAPED synthetic task list (mostly-DONE, with
//! UUID `:ID:` drawers and some nesting) — the motivating "1000-line file"
//! case.
//!
//! Run: `cargo run -p holon-org-format --example dense_measure`
//! Synthetic data only (repo is PUBLIC).

use std::collections::HashSet;
use std::path::Path;

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_api::types::TaskState;
use holon_org_format::AliasTable;
use holon_org_format::OrgBlockExt;
use holon_org_format::OrgDocumentExt;
use holon_org_format::OrgRenderer;
use holon_org_format::render_dense;

fn uuid_like(n: usize) -> String {
    // A synthetic 36-char UUID-shaped id (no real vault data).
    format!(
        "{:08x}-0000-4000-8000-{:012x}",
        n.wrapping_mul(2654435761),
        n
    )
}

fn main() {
    let file_id = EntityUri::block("11111111-0000-4000-8000-000000000000");
    let mut doc = Block::new_text(
        file_id.clone(),
        EntityUri::block("root"),
        "Synthetic task list",
    );
    doc.set_page(true);
    doc.set_file_title(Some("Synthetic task list".to_string()));
    doc.set_todo_keywords(Some(vec![
        TaskState::active("TODO"),
        TaskState::active("NEXT"),
        TaskState::done("DONE"),
    ]));

    // 120 tasks: 80% DONE, 20% active; every 5th is a child of the previous
    // top-level task (some nesting). Each carries a UUID :ID:.
    let mut blocks: Vec<Block> = Vec::new();
    let mut last_top: Option<EntityUri> = None;
    for i in 0..120 {
        let id = EntityUri::block(&uuid_like(i));
        let nested = i % 5 == 0 && last_top.is_some();
        let parent = if nested {
            last_top.clone().unwrap()
        } else {
            file_id.clone()
        };
        let mut b = Block::new_text(id.clone(), parent, format!("Task item number {i}"));
        let state = if i % 5 == 0 {
            TaskState::active("TODO")
        } else {
            TaskState::done("DONE")
        };
        b.set_task_state(Some(state));
        b.set_org_properties(Some(format!("{{\"ID\":\"{}\"}}", id.id())));
        if !nested {
            last_top = Some(id);
        }
        blocks.push(b);
    }

    let canonical = OrgRenderer::render_document(&doc, &blocks, Path::new("usage.org"), &file_id);

    let alias_table = AliasTable::assign(blocks.iter().map(|b| b.id.clone()));
    let gap_ids = HashSet::new();
    let dense = render_dense(&doc, &blocks, &file_id, &alias_table, &gap_ids);

    // chars/4 is the conventional GPT-family token estimate.
    let est = |s: &str| (s.chars().count() as f64 / 4.0).round() as usize;

    println!("blocks: {}", blocks.len());
    println!(
        "canonical (read_org_file): {} bytes, ~{} tokens",
        canonical.len(),
        est(&canonical)
    );
    println!(
        "dense projection:          {} bytes, ~{} tokens",
        dense.len(),
        est(&dense)
    );
    let ratio = dense.len() as f64 / canonical.len() as f64;
    println!(
        "dense / canonical: {:.1}% bytes ({:.1}% saved)",
        ratio * 100.0,
        (1.0 - ratio) * 100.0
    );
}
