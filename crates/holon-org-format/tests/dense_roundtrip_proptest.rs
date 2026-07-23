//! Round-trip property for the DENSE org projection (`crate::dense`).
//!
//! Property: for any generated block forest `F` with a per-query alias table
//! `T`, `parse_dense(render_dense(F, T))` recovers `F`'s structure and content
//! — titles, task states, and the parent/child tree (matched by alias). The
//! dense form must also actually be dense: `:ID:` drawer scaffolding replaced
//! by a trailing `{#alias}` token.
//!
//! This is the increment-1 red-first coverage for the dense syntax: it fails if
//! the token is not emitted, if a headline mis-parses, or if the alias⇄id
//! correspondence breaks. Synthetic data only (repo is PUBLIC).

use std::collections::HashMap;

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_api::types::TaskState;
use holon_org_format::AliasTable;
use holon_org_format::OrgBlockExt;
use holon_org_format::OrgDocumentExt;
use holon_org_format::parse_dense;
use holon_org_format::render_dense;
use proptest::prelude::*;

const ACTIVE_KW: &[&str] = &["TODO", "NEXT"];
const DONE_KW: &[&str] = &["DONE", "CANCELLED"];

/// A safe title word: lowercase letters + digits only. Lowercase guarantees it
/// can never collide with an (uppercase) TODO keyword, and the charset excludes
/// every org-structural char (`* # : [ ] { }`).
fn word() -> impl Strategy<Value = String> {
    prop::collection::vec(
        any::<usize>().prop_map(|i| {
            const CS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
            CS[i % CS.len()] as char
        }),
        1..=6,
    )
    .prop_map(|cs| cs.into_iter().collect())
}

fn title() -> impl Strategy<Value = String> {
    prop::collection::vec(word(), 1..=4).prop_map(|ws| ws.join(" "))
}

fn state() -> impl Strategy<Value = Option<TaskState>> {
    prop_oneof![
        3 => Just(None),
        2 => (0..ACTIVE_KW.len()).prop_map(|i| Some(TaskState::active(ACTIVE_KW[i]))),
        2 => (0..DONE_KW.len()).prop_map(|i| Some(TaskState::done(DONE_KW[i]))),
    ]
}

/// A generated node: title, task state, and a raw seed that selects its parent.
fn node() -> impl Strategy<Value = (String, Option<TaskState>, usize)> {
    (title(), state(), any::<usize>())
}

/// The document id every projection roots at.
fn file_id() -> EntityUri {
    EntityUri::block("dense-doc")
}

/// Build a block forest from generated nodes. Node `i`'s parent is chosen from
/// `{root} ∪ {0..i}` via its seed, clamped so depth never exceeds 3. Returns
/// the blocks in tree (pre-)order — parent always precedes child.
fn build_forest(nodes: &[(String, Option<TaskState>, usize)]) -> Vec<Block> {
    let fid = file_id();
    let mut ids: Vec<EntityUri> = Vec::with_capacity(nodes.len());
    let mut depth: Vec<usize> = Vec::with_capacity(nodes.len());
    let mut blocks: Vec<Block> = Vec::with_capacity(nodes.len());

    for (i, (t, st, seed)) in nodes.iter().enumerate() {
        let id = EntityUri::block(&format!("n{i}"));
        // Candidate parent index in 0..=i; == i means "root".
        let cand = seed % (i + 1);
        let (parent_id, d) = if cand == i || depth.get(cand).copied().unwrap_or(0) >= 3 {
            (fid.clone(), 0)
        } else {
            (ids[cand].clone(), depth[cand] + 1)
        };
        ids.push(id.clone());
        depth.push(d);

        let mut b = Block::new_text(id, parent_id, t.clone());
        b.set_task_state(st.clone());
        blocks.push(b);
    }
    blocks
}

fn doc_block() -> Block {
    let mut doc = Block::new_text(
        file_id(),
        EntityUri::block("dense-anchor"),
        "Dense Doc".to_string(),
    );
    doc.set_page(true);
    doc.set_file_title(Some("Dense Doc".to_string()));
    doc.set_todo_keywords(Some(vec![
        TaskState::active("TODO"),
        TaskState::active("NEXT"),
        TaskState::done("DONE"),
        TaskState::done("CANCELLED"),
    ]));
    doc
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 192, ..ProptestConfig::default() })]

    #[test]
    fn dense_projection_round_trips(nodes in prop::collection::vec(node(), 1..=8)) {
        let blocks = build_forest(&nodes);
        let fid = file_id();
        let table = AliasTable::assign(blocks.iter().map(|b| b.id.clone()));
        let doc = doc_block();

        let dense = render_dense(&doc, &blocks, &fid, &table);

        // Density property: no :ID: drawer line survived, and at least one
        // trailing token was emitted (the whole point of the projection).
        prop_assert!(
            !dense.contains(":ID:"),
            "dense projection still carries an :ID: drawer line:\n{dense}"
        );
        prop_assert!(
            dense.contains("{#"),
            "dense projection emitted no {{#alias}} token:\n{dense}"
        );

        let parsed = parse_dense(&dense).expect("dense projection must parse");
        prop_assert_eq!(
            parsed.blocks.len(),
            blocks.len(),
            "block count changed across dense round-trip:\n{}",
            dense
        );

        // Index parsed blocks by their alias, and a parse_id → alias map so we
        // can translate parent pointers back to aliases.
        let mut by_alias = HashMap::new();
        let mut parse_id_to_alias = HashMap::new();
        for db in &parsed.blocks {
            let alias = db
                .alias
                .clone()
                .expect("every projected block round-trips with an alias");
            parse_id_to_alias.insert(db.parse_id.as_str().to_string(), alias.clone());
            by_alias.insert(alias, db);
        }

        for original in &blocks {
            let alias = table
                .alias_of(&original.id)
                .expect("every block was aliased at projection");
            let db = by_alias
                .get(alias)
                .unwrap_or_else(|| panic!("alias {alias} missing from parsed projection"));

            prop_assert_eq!(
                db.block.org_title(),
                original.org_title(),
                "title diverged for alias {}",
                alias
            );
            prop_assert_eq!(
                db.block.task_state(),
                original.task_state(),
                "task state diverged for alias {}",
                alias
            );

            // Parent correspondence: a root (parent == file id) parses to no
            // parent row; a nested block's parent row carries the parent's alias.
            if original.parent_id == fid {
                prop_assert!(
                    db.parent_parse_id.is_none(),
                    "root block alias {} gained a parent on round-trip",
                    alias
                );
            } else {
                let expected_parent_alias = table
                    .alias_of(&original.parent_id)
                    .expect("parent was aliased");
                let got_parent_parse_id = db
                    .parent_parse_id
                    .as_ref()
                    .expect("nested block must have a parent row");
                let got_parent_alias = parse_id_to_alias
                    .get(got_parent_parse_id.as_str())
                    .expect("parent parse_id resolves to a parsed row");
                prop_assert_eq!(
                    got_parent_alias,
                    expected_parent_alias,
                    "parent diverged for alias {}",
                    alias
                );
            }
        }
    }
}
