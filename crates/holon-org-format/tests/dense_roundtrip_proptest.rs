//! Round-trip property for the DENSE org projection (`crate::dense`).
//!
//! Property: for any generated block forest `F` with a per-query alias table
//! `T`, `parse_dense(render_dense(F, T))` recovers `F`'s structure and content
//! — titles, task states, tags, drawer properties, and the parent/child tree
//! (matched by alias). The
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

const TAG_POOL: &[&str] = &["decision", "option", "ops_2"];
const PROPERTY_KEYS: &[&str] = &["choose", "recommend", "option", "asked-by"];

fn tags() -> impl Strategy<Value = Vec<String>> {
    prop::sample::subsequence(TAG_POOL, 0..=TAG_POOL.len())
        .prop_map(|ts| ts.into_iter().map(str::to_string).collect())
}

/// Drawer properties whose values carry the characters a title-side token
/// grammar would trip over (`{`, `:`, `#`).
fn properties() -> impl Strategy<Value = Vec<(String, String)>> {
    prop::sample::subsequence(PROPERTY_KEYS, 0..=2).prop_flat_map(|keys| {
        let n = keys.len();
        prop::collection::vec(
            prop_oneof![
                title(),
                Just("1..3".to_string()),
                Just("a {#1} :x:".to_string()),
            ],
            n,
        )
        .prop_map(move |values| {
            keys.iter()
                .map(|k| k.to_string())
                .zip(values)
                .collect::<Vec<_>>()
        })
    })
}

/// Titles carrying the characters the headline grammar gives meaning to: a
/// trailing tag-shaped group, colons, a mid-title `{#n}`, and `#`.
fn org_shaped_title() -> impl Strategy<Value = String> {
    prop_oneof![
        title(),
        (title(), word()).prop_map(|(t, w)| format!("{t} :{w}:")),
        (title(), word(), word()).prop_map(|(t, a, b)| format!("{t} :{a}:{b}:")),
        title().prop_map(|t| format!("{t}:")),
        (title(), 0u8..10).prop_map(|(t, n)| format!("{t} {{#{n}}}")),
        (title(), word()).prop_map(|(t, w)| format!("{t} ::{w} #{w}")),
        (word(), word()).prop_map(|(a, b)| format!("{a} :{a} {b}:")),
    ]
}

/// Values a headline drawer holds only as a literal, next to plain ones.
fn any_value() -> impl Strategy<Value = String> {
    prop_oneof![
        title(),
        "\\PC{0,8}",
        prop::sample::select(vec!["", " ", " a ", "a\nb", "\"q\"", ":x:", "{#1}"])
            .prop_map(str::to_string),
    ]
}

struct GenNode {
    title: String,
    state: Option<TaskState>,
    tags: Vec<String>,
    properties: Vec<(String, String)>,
    parent_seed: usize,
}

/// A generated node; `parent_seed` selects its parent.
fn node() -> impl Strategy<Value = GenNode> {
    (title(), state(), tags(), properties(), any::<usize>()).prop_map(
        |(title, state, tags, properties, parent_seed)| GenNode {
            title,
            state,
            tags,
            properties,
            parent_seed,
        },
    )
}

impl std::fmt::Debug for GenNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?} {:?} {:?} {:?} ^{}",
            self.title,
            self.state.as_ref().map(|s| &s.keyword),
            self.tags,
            self.properties,
            self.parent_seed
        )
    }
}

/// The document id every projection roots at.
fn file_id() -> EntityUri {
    EntityUri::block("dense-doc")
}

/// Build a block forest from generated nodes. Node `i`'s parent is chosen from
/// `{root} ∪ {0..i}` via its seed, clamped so depth never exceeds 3. Returns
/// the blocks in tree (pre-)order — parent always precedes child.
fn build_forest(nodes: &[GenNode]) -> Vec<Block> {
    let fid = file_id();
    let mut ids: Vec<EntityUri> = Vec::with_capacity(nodes.len());
    let mut depth: Vec<usize> = Vec::with_capacity(nodes.len());
    let mut blocks: Vec<Block> = Vec::with_capacity(nodes.len());

    for (i, n) in nodes.iter().enumerate() {
        let seed = &n.parent_seed;
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

        let mut b = Block::new_text(id, parent_id, n.title.clone());
        b.set_task_state(n.state.clone());
        b.tags = holon_api::Tags::from_tag_iter(n.tags.clone());
        for (k, v) in &n.properties {
            b.set_property(k, holon_api::Value::String(v.clone()));
        }
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

/// `dense` parsed and rendered again with the same aliases.
fn rerender(dense: &str, doc: &Block) -> String {
    let fid = file_id();
    let parsed = parse_dense(dense).expect("dense projection must parse");
    let blocks: Vec<Block> = parsed
        .blocks
        .iter()
        .map(|db| {
            let mut b = db.block.clone();
            if db.parent_parse_id.is_none() {
                b.parent_id = fid.clone();
            }
            b
        })
        .collect();
    let table = AliasTable::from_pairs(parsed.blocks.iter().map(|db| {
        (
            db.alias
                .clone()
                .expect("every projected block carries an alias"),
            db.block.id.clone(),
        )
    }))
    .expect("aliases are distinct");
    render_dense(
        doc,
        &blocks,
        &fid,
        &table,
        &std::collections::HashSet::new(),
    )
    .expect("dense re-render")
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 192,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn dense_projection_round_trips(nodes in prop::collection::vec(node(), 1..=8)) {
        let blocks = build_forest(&nodes);
        let fid = file_id();
        let table = AliasTable::assign(blocks.iter().map(|b| b.id.clone()));
        let doc = doc_block();

        // No holes in this generator (every parent is present), so no gap markers.
        let gap_ids = std::collections::HashSet::new();
        let dense = render_dense(&doc, &blocks, &fid, &table, &gap_ids).expect("dense render");

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
            prop_assert_eq!(
                db.block.tags(),
                original.tags(),
                "tags diverged for alias {}:\n{}",
                alias,
                dense
            );
            prop_assert_eq!(
                db.block.drawer_properties(),
                original.drawer_properties(),
                "drawer properties diverged for alias {}:\n{}",
                alias,
                dense
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

    #[test]
    fn dense_projection_is_a_render_fixed_point(
        nodes in prop::collection::vec(
            (org_shaped_title(), state(), tags(), prop::collection::btree_map("p[a-z-]{0,5}", any_value(), 0..3), any::<usize>())
                .prop_map(|(title, state, tags, properties, parent_seed)| GenNode {
                    title,
                    state,
                    tags,
                    properties: properties.into_iter().collect(),
                    parent_seed,
                }),
            1..=8,
        )
    ) {
        let blocks = build_forest(&nodes);
        let fid = file_id();
        let table = AliasTable::assign(blocks.iter().map(|b| b.id.clone()));
        let doc = doc_block();
        let gap_ids = std::collections::HashSet::new();
        let dense = render_dense(&doc, &blocks, &fid, &table, &gap_ids).expect("dense render");

        // A stored title ending in a tag-shaped run reads back with that run
        // as tags, so the first render need not be the fixed point; the
        // projection of what it parses to must be.
        let once = rerender(&dense, &doc);
        let again = rerender(&once, &doc);
        prop_assert_eq!(&again, &once, "render . parse is not the identity on:\n{}", once);
    }
}
