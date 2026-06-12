//! Proptest strategy generators for PBT transitions.

use std::collections::HashSet;

use proptest::prelude::*;

use holon_api::block::Block;
use holon_api::{ContentType, EntityUri, QueryLanguage, SourceLanguage, TaskState, Value};
use holon_orgmode::models::OrgBlockExt;

use super::query::{QuerySource, QueryTable, TestQuery};
use super::reference_state::{VALID_PROFILE_YAMLS, valid_render_expression_strings};
use super::types::Mutation;
use holon_api::predicate::Predicate;

use std::collections::HashMap;

/// A set of TODO keywords generated per test case.
/// Drives both the `#+TODO:` org header and the task_state mutation generator.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TodoKeywordSet(pub Vec<TaskState>);

impl TodoKeywordSet {
    /// Render as org header line, e.g. `#+TODO: TODO DOING | DONE CANCELLED`
    pub fn to_org_header(&self) -> String {
        let active: Vec<&str> = self
            .0
            .iter()
            .filter(|ts| ts.is_active())
            .map(|ts| ts.keyword.as_str())
            .collect();
        let done: Vec<&str> = self
            .0
            .iter()
            .filter(|ts| ts.is_done())
            .map(|ts| ts.keyword.as_str())
            .collect();
        format!("#+TODO: {} | {}", active.join(" "), done.join(" "))
    }

    /// All keyword strings (for sampling in mutations).
    pub fn all_keywords(&self) -> Vec<String> {
        self.0.iter().map(|ts| ts.keyword.clone()).collect()
    }
}

pub fn todo_keyword_set_strategy() -> impl Strategy<Value = TodoKeywordSet> {
    prop::collection::vec(
        prop_oneof![
            Just(TaskState::active("TODO")),
            Just(TaskState::active("DOING")),
            Just(TaskState::active("STARTED")),
            Just(TaskState::active("NEXT")),
            Just(TaskState::active("WAITING")),
        ],
        1..=3,
    )
    .prop_flat_map(|active| {
        prop::collection::vec(
            prop_oneof![
                Just(TaskState::done("DONE")),
                Just(TaskState::done("CANCELLED")),
                Just(TaskState::done("CLOSED")),
            ],
            1..=2,
        )
        .prop_map(move |done| TodoKeywordSet([active.clone(), done].concat()))
    })
}

/// Phase 3 extended-generation toggle (`HOLON_PBT_EXTENDED_GEN=1`).
///
/// Default generators are deliberately ASCII-only and non-empty, which hides
/// the byte-offset-vs-char-count bug class entirely. The extended arms add
/// multi-byte content, empty/whitespace-only strings, and org-special
/// prefixes — gated behind this env so the blessed default gates stay green
/// while extended-slice findings are triaged one axis at a time. Promote an
/// arm into the defaults once its slice is green.
pub fn extended_gen_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var("HOLON_PBT_EXTENDED_GEN").as_deref() == Ok("1");
        if on {
            eprintln!("[HOLON_PBT_EXTENDED_GEN] extended generators ACTIVE (Phase 3)");
        }
        on
    })
}

/// Characters spanning all UTF-8 widths plus ASCII filler. The 2/3/4-byte
/// codepoints are the payload: any path that converts byte offsets to
/// keystroke counts (or slices content at a char boundary it computed in
/// the other unit) breaks on these.
fn extended_char() -> BoxedStrategy<char> {
    prop_oneof![
        2 => proptest::char::range('a', 'z'),
        1 => Just(' '),
        2 => prop_oneof![Just('é'), Just('ß'), Just('ñ')],   // 2-byte
        2 => prop_oneof![Just('€'), Just('中'), Just('日')], // 3-byte
        2 => prop_oneof![Just('😀'), Just('🎉')],            // 4-byte emoji
    ]
    .boxed()
}

/// Single-line extended content: multi-byte runs, empty, whitespace-only,
/// and org-special prefixes (`*`, `[[…]]`, `:PROPERTIES:`, `#+`) that a user
/// can legitimately type but the org renderer must round-trip.
fn extended_content_arm() -> BoxedStrategy<String> {
    prop_oneof![
        4 => prop::collection::vec(extended_char(), 1..=12)
            .prop_map(|cs| cs.into_iter().collect::<String>()),
        1 => Just(String::new()),
        1 => prop_oneof![Just("   ".to_string()), Just("\t".to_string())],
        // Org-special spellings. `:PROPERTIES:{tail}` (incl. the bare
        // trailing-tag-group form) is BY-DESIGN org tag syntax — the ref
        // model mirrors it via `split_headline_tags` in
        // `normalize_content_for_org_roundtrip`; pinned in
        // holon-org-format/tests/properties_prefix_headline_repro.rs.
        2 => ("[A-Za-z0-9 ]{0,12}", 0..4u8).prop_map(|(tail, kind)| match kind {
            0 => format!("* {tail}"),
            1 => format!("[[{tail}]]"),
            2 => format!("#+ {tail}"),
            _ => format!(":PROPERTIES:{tail}"),
        }),
    ]
    .boxed()
}

/// Generate single-line block content for headlines.
/// Headlines must be single-line because the org parser treats newlines in
/// headline text as content boundaries — multi-line headlines cause
/// `:PROPERTIES:` drawers to be embedded in the content.
pub fn content_strategy() -> BoxedStrategy<String> {
    let base = "[A-Z][a-zA-Z0-9 ]{0,20}".prop_map(|s| s).boxed();
    prop_oneof![6 => base, 4 => extended_content_arm()].boxed()
}

/// Same as `content_strategy` but for edit mutations (lowercase start).
pub fn edit_content_strategy() -> BoxedStrategy<String> {
    let base = prop_oneof![
        7 => "[a-zA-Z][a-zA-Z0-9 ]{0,20}".prop_map(|s| s),
        3 => (
            "[a-z][a-zA-Z0-9 ]{3,15}",
            prop::collection::vec("[a-z][a-zA-Z0-9 ]{3,15}", 1..=3),
        )
            .prop_map(|(first, rest)| {
                let mut lines = vec![first];
                lines.extend(rest);
                lines.join("\n")
            }),
    ]
    .boxed();
    prop_oneof![6 => base, 4 => extended_content_arm()].boxed()
}

/// Per-keystroke typing text (`TypeChars`). Always non-empty — a
/// zero-keystroke transition is unobservable on both sides. Extended mode
/// mixes multi-byte codepoints into the keystroke stream, stressing the
/// byte-vs-keystroke conversions in the split/caret path.
pub fn typing_text_strategy() -> BoxedStrategy<String> {
    let base = "[a-z]{1,4}".prop_map(|s| s).boxed();
    prop_oneof![
        5 => base,
        5 => prop::collection::vec(extended_char(), 1..=4)
            .prop_map(|cs| cs.into_iter().collect::<String>()),
    ]
    .boxed()
}

/// Peer-edit content (`PeerEdit::{Create,Update}`). Extended mode adds
/// multi-byte and org-special content arriving from a remote peer.
pub fn peer_content_strategy() -> BoxedStrategy<String> {
    let base = "[a-z]{4,8}".prop_map(|s| s).boxed();
    prop_oneof![6 => base, 4 => extended_content_arm()].boxed()
}

/// Externally-added block content (`BulkExternalAdd`). Extended mode adds
/// the full extended-content arm via the external write path.
pub fn bulk_content_strategy() -> BoxedStrategy<String> {
    let base = "[a-zA-Z][a-zA-Z0-9 ]{0,20}".prop_map(|s| s).boxed();
    prop_oneof![6 => base, 4 => extended_content_arm()].boxed()
}

/// Build the blocks for an index.org heading with a query source + render source.
///
/// Returns the generated `Block`s directly (a heading + a query source + a
/// render source). The seeding transition decides how to materialise them
/// against the SUT — serialise to org text for a Turso/org wiring, or write
/// them straight into the Loro doc for a no-Turso wiring — so the generator
/// itself no longer renders to a string.
fn index_org_blocks(
    headline: &str,
    id: &str,
    query_lang: QueryLanguage,
    query_source: &str,
    render_expr: &str,
) -> Vec<Block> {
    let doc_uri = EntityUri::block("gen-placeholder");
    let heading_uri = EntityUri::block(id);

    let mut heading = Block::new_text(heading_uri.clone(), doc_uri.clone(), headline);
    heading.set_property("ID", Value::String(id.to_string()));

    let mut query_block = Block::new_source(
        EntityUri::block(&format!("{id}::src::0")),
        heading_uri.clone(),
        SourceLanguage::Query(query_lang).to_string(),
        query_source,
    );
    query_block.set_sequence(1);

    let mut render_entity = Block::new_source(
        EntityUri::block(&format!("{id}::render::0")),
        heading_uri,
        "render",
        render_expr,
    );
    render_entity.set_sequence(2);

    vec![heading, query_block, render_entity]
}

/// Generate `(filename, content)` for a `WriteOrgFile` transition.
///
/// `allow_index_override`: when `false`, only emits non-index files
/// (`<name>_<n>.org` plain files and entity-profile files). When `true`,
/// also emits the four `index.org` variants that completely replace the
/// default root layout — disable while reproducing layout-sensitive bugs
/// where `state_toggle` / `editable_text` need to be present in the main
/// panel's render expression.
pub fn generate_org_file_content_with_keywords(
    keyword_set: Option<TodoKeywordSet>,
    allow_index_override: bool,
) -> BoxedStrategy<(String, Vec<Block>)> {
    use proptest::collection::vec as prop_vec;

    let ks = keyword_set.clone();
    // Generate headlines with optional task states: (headline, id, maybe_task_state_index)
    // ~50% of headlines get a random task keyword when a keyword_set is present.
    //
    // Headline regex requires the *second* character to be lowercase so the
    // first word cannot accidentally match an all-caps TODO keyword (TODO,
    // DOING, DONE, NEXT, STARTED, WAITING, CANCELLED, CLOSED, …). Without
    // this, a randomly generated headline like `TODO Foo` collides with the
    // org parser's task-state extraction: the actual parser respects the
    // doc's `#+TODO:` set (so `TODO` may stay as content), while the
    // reference parser uses the always-on default set — divergence.
    let regular_file = (
        "[a-z_]+_[0-9]+\\.org",
        prop_vec(
            (
                // Seeded headline content — extended arms (multi-byte,
                // empty, org-special prefixes) stress the parser boundary.
                content_strategy(),
                "[a-z0-9-]+",
                prop::bool::ANY,
                // `make_requires`: when true (and a prior sibling exists), this
                // heading gets a `:REQUIRES:` org-edna dependency on the
                // immediately-preceding sibling. Exercises the `requires` edge
                // field end-to-end (parser → Loro/SQL junction → projection →
                // renderer) — the path that was previously never generated.
                prop::bool::ANY,
                // Parent selector (extended-gen axis 2): heading `i` parents to
                // `sel % (i + 1)` — 0 = doc root, k = heading `k-1`. Same
                // predecessor trick as `make_requires`: only already-built
                // headings are eligible, so the tree is well-founded by
                // construction. Default gen ignores this (flat files).
                prop::num::u8::ANY,
            ),
            1..=5,
        ),
    )
        .prop_map(move |(filename, headings)| {
            let doc_uri = EntityUri::block("gen-placeholder");

            let all_keywords: Vec<String> = ks
                .as_ref()
                .map(|set| set.all_keywords())
                .unwrap_or_default();
            // Sibling ids in document order, so a heading can depend on its
            // predecessor by id. Collected up front because the dependency
            // target (`ids[i-1]`) must be known while building block `i`.
            let ids: Vec<String> = headings.iter().map(|(_, id, _, _, _)| id.clone()).collect();
            // First pass: resolve each heading's parent. Flat (doc root) by
            // default; under extended gen, heading `i` parents to
            // `sel % (i + 1)` (0 = root, k = heading k-1) — well-founded by
            // construction since only earlier headings are eligible.
            // Duplicate random ids skip nesting rather than self-parent.
            let parents: Vec<EntityUri> = headings
                .iter()
                .enumerate()
                .map(
                    |(i, (_, id, _, _, parent_sel))| match *parent_sel as usize % (i + 1) {
                        0 => doc_uri.clone(),
                        k if ids[k - 1] != *id => EntityUri::block(&ids[k - 1]),
                        _ => doc_uri.clone(),
                    },
                )
                .collect();
            let blocks: Vec<Block> = headings
                .into_iter()
                .enumerate()
                .map(|(i, (headline, id, make_task, make_requires, _))| {
                    let mut b =
                        Block::new_text(EntityUri::block(&id), parents[i].clone(), &headline);
                    b.set_property("ID", Value::String(id.clone()));
                    // Assign a task keyword to ~50% of headlines when keywords exist.
                    // Cycle through keywords using the index for variety.
                    if make_task && !all_keywords.is_empty() {
                        let kw = &all_keywords[i % all_keywords.len()];
                        b.set_task_state(Some(TaskState::from_keyword(kw)));
                    }
                    // Single-element requires (depend on the previous TRUE
                    // sibling — same parent, so nesting stays orthogonal to
                    // the requires axis) — kept to one entry so the
                    // order-insensitive junction read can't false-diff against
                    // the parsed order. Skip self-refs from duplicate ids.
                    // Stored as a `block:` URI to match the parser boundary.
                    if make_requires && i > 0 && ids[i - 1] != id && parents[i] == parents[i - 1] {
                        b.requires = vec![EntityUri::block(&ids[i - 1])];
                    }
                    b
                })
                .collect();
            (filename, blocks)
        });

    // Layout query sources are now emitted by `TestQuery::compile_layout_for`
    // so the SUT runs a query the reference can recover (via
    // `QuerySource::recognize`) and `evaluate` identically. Each variant pairs a
    // faithful `QuerySource` with a render template of known interactivity:
    // PRQL `from children` (navigation-blind direct children — empty under the
    // layout block) with an interactive template; GQL all-blocks / var-length
    // descendants and SQL direct-children with static templates.
    //
    // The root heading carries the FIXED `:ID: root-layout` — the layout root
    // is the hardcoded `block:root-layout` (`root_layout_block_uri()`), so a
    // user index.org only takes over the layout when its root heading keeps
    // that well-known ID. A random ID would leave the override silently
    // ignored (the seeded default layout keeps rendering) — that's the
    // contract pinned by the 2026-06-10 layout-override finding.
    fn root_layout_bare_id() -> String {
        holon_api::ROOT_LAYOUT_BLOCK_ID
            .strip_prefix("block:")
            .expect("ROOT_LAYOUT_BLOCK_ID is block-schemed")
            .to_string()
    }
    let index_file_prql = "[A-Z][a-zA-Z0-9 ]{0,15}".prop_map(|headline| {
        let id = root_layout_bare_id();
        let (src, lang) = TestQuery::layout(QuerySource::DirectChildren {
            context: EntityUri::block(&id),
        })
        .compile_layout_for(QueryLanguage::HolonPrql);
        let blocks = index_org_blocks(
            &headline,
            &id,
            lang,
            &format!("{src}\n"),
            "list(#{item_template: row(state_toggle(col(\"task_state\")), editable_text(col(\"content\")))})",
        );
        ("index.org".to_string(), blocks)
    });

    let index_file_gql = "[A-Z][a-zA-Z0-9 ]{0,15}".prop_map(|headline| {
        let id = root_layout_bare_id();
        let (src, lang) =
            TestQuery::layout(QuerySource::AllBlocks).compile_layout_for(QueryLanguage::HolonGql);
        let blocks = index_org_blocks(
            &headline,
            &id,
            lang,
            &format!("{src}\n"),
            "list(#{item_template: row(text(\"node\"))})",
        );
        ("index.org".to_string(), blocks)
    });

    let index_file_gql_varlen = "[A-Z][a-zA-Z0-9 ]{0,15}".prop_map(|headline| {
        let id = root_layout_bare_id();
        let (src, lang) = TestQuery::layout(QuerySource::DescendantsOfAny {
            min_depth: 1,
            max_depth: 3,
        })
        .compile_layout_for(QueryLanguage::HolonGql);
        let blocks = index_org_blocks(
            &headline,
            &id,
            lang,
            &format!("{src}\n"),
            "list(#{item_template: row(text(\"varlen\"))})",
        );
        ("index.org".to_string(), blocks)
    });

    let index_file_sql = "[A-Z][a-zA-Z0-9 ]{0,15}".prop_map(|headline| {
        let id = root_layout_bare_id();
        let (src, lang) = TestQuery::layout(QuerySource::DirectChildren {
            context: EntityUri::block(&id),
        })
        .compile_layout_for(QueryLanguage::HolonSql);
        let blocks = index_org_blocks(
            &headline,
            &id,
            lang,
            &format!("{src}\n"),
            "list(#{item_template: row(text(\"sql node\"))})",
        );
        ("index.org".to_string(), blocks)
    });

    // Tree view with virtual child — exercises the trailing-slot creation
    // path. `creation_slot: true` triggers `build_trailing_slot`;
    // `virtual_parent` is deliberately omitted — the builder falls back to
    // `ba.ctx.row().get("id")` (the focused block's id).
    let index_file_tree = "[A-Z][a-zA-Z0-9 ]{0,15}".prop_map(|headline| {
        let id = root_layout_bare_id();
        let (src, lang) = TestQuery::layout(QuerySource::DirectChildren {
            context: EntityUri::block(&id),
        })
        .compile_layout_for(QueryLanguage::HolonPrql);
        let blocks = index_org_blocks(
            &headline,
            &id,
            lang,
            &format!("{src}\n"),
            "tree(#{parent_id: col(\"parent_id\"), sortkey: col(\"sequence\"), item_template: render_entity(), creation_slot: true})",
        );
        ("index.org".to_string(), blocks)
    });

    let file_with_profile = (
        "[a-z_]+_[0-9]+\\.org",
        "[A-Z][a-zA-Z0-9 ]{0,15}",
        "[a-z0-9-]+",
        prop::sample::select(VALID_PROFILE_YAMLS.to_vec()),
    )
        .prop_map(|(filename, headline, id, yaml)| {
            let doc_uri = EntityUri::block("gen-placeholder");
            let heading_uri = EntityUri::block(&id);

            let mut heading = Block::new_text(heading_uri.clone(), doc_uri.clone(), &headline);
            heading.set_property("ID", Value::String(id.clone()));

            let mut profile_block = Block::new_source(
                EntityUri::block(&format!("{id}::src::0")),
                heading_uri,
                "holon_entity_profile_yaml",
                &*yaml,
            );
            profile_block.set_sequence(1);

            let blocks = vec![heading, profile_block];
            (filename, blocks)
        });

    // Shared-tree mount: a headline with `:share-role: mount` and
    // `:shared-tree-id: <uuid>` in its property drawer, plus 1–3 children
    // that carry the same `:shared-tree-id:`. This shape exists in
    // production (see `crates/holon/src/sync/shared_tree.rs`,
    // `crates/holon/src/sync/loro_share_backend.rs`) but is otherwise
    // unreachable from the PBT — no transition creates a mount node,
    // and the regular-file generator above never emits drawer keys
    // outside `:ID:` / `:TODO:` / etc.
    //
    // Why this generator exists: the May-2026 loop on
    // `Phase 6: Flow Optimization.org` was a render-fixed-point
    // violation specific to mount-node ingestion (the SQL row for the
    // mount block was missing the `ID` property, sort_keys on tied
    // siblings drifted, and the renderer's output disagreed with the
    // disk file — feeding the FSEvent → on_file_changed loop). Without
    // this generator the bug class can't be surfaced by
    // `inv-org-render-fixed-point`.
    let shared_tree_mount_file = (
        "[a-z_]+_[0-9]+\\.org",
        "[A-Z][a-z][a-zA-Z0-9 ]{0,15}", // mount headline
        "[a-z0-9-]+",                   // mount block id
        "[a-z0-9-]+",                   // shared-tree-id
        prop_vec(("[A-Z][a-z][a-zA-Z0-9 ]{0,19}", "[a-z0-9-]+"), 1..=3),
    )
        .prop_map(|(filename, mount_headline, mount_id, tree_id, children)| {
            let doc_uri = EntityUri::block("gen-placeholder");
            let mount_uri = EntityUri::block(&mount_id);

            let mut mount = Block::new_text(mount_uri.clone(), doc_uri.clone(), &mount_headline);
            mount.set_property("ID", Value::String(mount_id.clone()));
            mount.set_property("share-role", Value::String("mount".to_string()));
            mount.set_property("shared-tree-id", Value::String(tree_id.clone()));

            let mut blocks = vec![mount];
            for (headline, id) in children {
                let mut child =
                    Block::new_text(EntityUri::block(&id), mount_uri.clone(), &headline);
                child.set_property("ID", Value::String(id.clone()));
                child.set_property("shared-tree-id", Value::String(tree_id.clone()));
                blocks.push(child);
            }

            (filename, blocks)
        });

    // Shared-tree mount files are gated by `HOLON_PBT_SHARED_TREE_MOUNT=1`.
    // The reference model doesn't yet track `share-role` / `shared-tree-id`
    // properties or the mount-children-belong-to-mount hierarchy, so leaving
    // the generator on by default makes `assert_blocks_equivalent` (Backend
    // diverged from reference) fail on every run that hits this branch.
    // Flip the env var to actively hunt mount-shape regressions (e.g. the
    // May-2026 `Phase 6: Flow Optimization` loop class); leave it off to
    // keep CI green.
    let mount_enabled = std::env::var("HOLON_PBT_SHARED_TREE_MOUNT").ok().as_deref() == Some("1");

    if allow_index_override {
        if mount_enabled {
            prop_oneof![
                3 => regular_file,
                2 => index_file_prql,
                1 => index_file_gql,
                1 => index_file_gql_varlen,
                1 => index_file_sql,
                1 => index_file_tree,
                1 => file_with_profile,
                2 => shared_tree_mount_file,
            ]
            .boxed()
        } else {
            let _ = shared_tree_mount_file; // hold onto strategy without firing
            prop_oneof![
                3 => regular_file,
                2 => index_file_prql,
                1 => index_file_gql,
                1 => index_file_gql_varlen,
                1 => index_file_sql,
                1 => index_file_tree,
                1 => file_with_profile,
            ]
            .boxed()
        }
    } else if extended_gen_enabled() {
        // Extended-gen axis 4: profile-bearing files in the no-overrides
        // configuration. The test profile YAMLs render just
        // `row(editable_text(...))` — no state_toggle — so the ref model
        // must track the active profile per rendered block. Triaged on the
        // extended slice before promotion.
        prop_oneof![
            8 => regular_file,
            1 => file_with_profile,
        ]
        .boxed()
    } else {
        // Profile-bearing files override the default block entity profile,
        // and the test's profile YAMLs render just `row(editable_text(...))`
        // — no state_toggle. We keep them out of the no-overrides
        // configuration so the default `assets/default/types/block_profile.yaml`
        // (which has the state_toggle variant) stays in effect.
        regular_file.boxed()
    }
}

pub fn generate_directory_path() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z_]+_[0-9]+".prop_map(|name| name),
        ("[a-z_]+", "[a-z_]+_[0-9]+").prop_map(|(parent, child)| format!("{}/{}", parent, child)),
        ("[a-z_]+", "[a-z_]+", "[a-z_]+_[0-9]+").prop_map(|(a, b, c)| format!("{}/{}/{}", a, b, c)),
    ]
}

pub fn generate_mutation(
    next_id: usize,
    existing_block_ids: Vec<EntityUri>,
    text_block_ids: Vec<EntityUri>,
    doc_uris: Vec<EntityUri>,
    no_content_update_ids: HashSet<EntityUri>,
) -> impl Strategy<Value = Mutation> {
    let mut valid_parent_ids_for_text = doc_uris.clone();
    valid_parent_ids_for_text.extend(existing_block_ids.iter().cloned());

    let valid_parent_ids_for_source = text_block_ids;

    let create_text = (
        edit_content_strategy(),
        prop::sample::select(valid_parent_ids_for_text),
        prop::option::of((
            prop::sample::select(vec![
                "effort",
                "story_points",
                "estimate",
                "reviewer",
                "column-order",
                "collapse-to",
                "ideal-width",
                "column-priority",
            ]),
            "[a-zA-Z0-9]{1,10}",
        )),
    )
        .prop_map(move |(content, parent_id, custom_prop)| {
            let mut fields: HashMap<String, Value> = [
                ("content".to_string(), Value::String(content)),
                ("content_type".to_string(), ContentType::Text.into()),
            ]
            .into_iter()
            .collect();
            if let Some((prop_name, prop_value)) = custom_prop {
                fields.insert(prop_name.to_string(), Value::String(prop_value));
            }
            Mutation::Create {
                entity: "block".to_string(),
                id: EntityUri::block(&format!("block-{}", next_id)),
                parent_id,
                fields,
            }
        });

    if valid_parent_ids_for_source.is_empty() {
        return create_text.boxed();
    }

    let create_source = (
        prop::sample::select(vec!["python", "rust", "elisp", "shell"]),
        "[a-zA-Z_][a-zA-Z0-9_ \n]{5,50}",
        prop::sample::select(valid_parent_ids_for_source),
    )
        .prop_map(
            move |(language, source_content, parent_id)| Mutation::Create {
                entity: "block".to_string(),
                id: EntityUri::block(&format!("block-{}", next_id)),
                parent_id,
                fields: [
                    ("content".to_string(), Value::String(source_content)),
                    ("content_type".to_string(), ContentType::Source.into()),
                    (
                        "source_language".to_string(),
                        Value::String(language.to_string()),
                    ),
                ]
                .into_iter()
                .collect(),
            },
        );

    let create = prop_oneof![3 => create_text, 1 => create_source];

    let ids = existing_block_ids;

    let updatable_content_ids: Vec<EntityUri> = ids
        .iter()
        .filter(|id| !no_content_update_ids.contains(id))
        .cloned()
        .collect();

    let update_content = if updatable_content_ids.is_empty() {
        Just(Mutation::Update {
            entity: "block".to_string(),
            id: ids[0].clone(),
            fields: [("content".to_string(), Value::String("fallback".to_string()))]
                .into_iter()
                .collect(),
        })
        .boxed()
    } else {
        (
            prop::sample::select(updatable_content_ids),
            edit_content_strategy(),
        )
            .prop_map(|(id, new_content)| Mutation::Update {
                entity: "block".to_string(),
                id,
                fields: [("content".to_string(), Value::String(new_content))]
                    .into_iter()
                    .collect(),
            })
            .boxed()
    };

    // Custom properties live in :PROPERTIES: drawers, which only exist on
    // headings (text blocks). Source blocks cannot carry org properties.
    let prop_target_ids: Vec<EntityUri> = ids
        .iter()
        .filter(|id| !no_content_update_ids.contains(id))
        .cloned()
        .collect();

    let update = if prop_target_ids.is_empty() {
        update_content.boxed()
    } else {
        let update_custom_prop = (
            prop::sample::select(prop_target_ids),
            prop::sample::select(vec![
                "effort",
                "story_points",
                "column-order",
                "collapse-to",
                "ideal-width",
                "column-priority",
            ]),
            "[a-zA-Z0-9]{1,10}",
        )
            .prop_map(|(id, prop_name, prop_value)| Mutation::Update {
                entity: "block".to_string(),
                id,
                fields: [(prop_name.to_string(), Value::String(prop_value))]
                    .into_iter()
                    .collect(),
            });

        prop_oneof![2 => update_content, 1 => update_custom_prop].boxed()
    };

    let delete = prop::sample::select(ids).prop_map(|id| Mutation::Delete {
        entity: "block".to_string(),
        id,
    });

    prop_oneof![3 => create, 2 => update, 1 => delete].boxed()
}

pub fn generate_test_query() -> impl Strategy<Value = TestQuery> {
    let columns = Just(vec![
        "id".to_string(),
        "content".to_string(),
        "content_type".to_string(),
        "source_language".to_string(),
        "source_name".to_string(),
        "parent_id".to_string(),
    ]);
    let predicates = prop::collection::vec(generate_predicate(), 0..=2);

    (columns, predicates).prop_map(|(columns, predicates)| TestQuery {
        table: QueryTable::Blocks,
        columns,
        predicates,
        source: crate::pbt::query::QuerySource::AllBlocks,
    })
}

pub fn generate_predicate() -> impl Strategy<Value = Predicate> {
    prop_oneof![
        Just(Predicate::Ne {
            field: "content".into(),
            value: Value::String("".into()),
        }),
        Just(Predicate::Eq {
            field: "content_type".into(),
            value: Value::String("text".into()),
        }),
        Just(Predicate::Eq {
            field: "content_type".into(),
            value: Value::String("source".into()),
        }),
        Just(Predicate::IsNotNull("source_language".into())),
    ]
}

/// Generate a query language, weighted towards PRQL (primary path).
/// Extended-gen axis 4 adds GQL (the third user-reachable language);
/// triaged on the extended slice before promotion.
pub fn generate_query_language() -> BoxedStrategy<QueryLanguage> {
    if extended_gen_enabled() {
        return prop_oneof![
            4 => Just(QueryLanguage::HolonPrql),
            3 => Just(QueryLanguage::HolonSql),
            2 => Just(QueryLanguage::HolonGql),
        ]
        .boxed();
    }
    prop_oneof![
        5 => Just(QueryLanguage::HolonPrql),
        3 => Just(QueryLanguage::HolonSql),
    ]
    .boxed()
}

/// Generate content or task_state mutations for layout headline blocks.
pub fn generate_layout_headline_mutation(
    ids: Vec<EntityUri>,
    keyword_set: Option<TodoKeywordSet>,
) -> impl Strategy<Value = Mutation> {
    let content_mutation =
        (prop::sample::select(ids.clone()), content_strategy()).prop_map(|(id, content)| {
            Mutation::Update {
                entity: "block".to_string(),
                id,
                fields: [("content".to_string(), Value::String(content))]
                    .into_iter()
                    .collect(),
            }
        });

    if let Some(ks) = keyword_set {
        let mut keywords_with_none: Vec<Option<String>> =
            ks.all_keywords().into_iter().map(Some).collect();
        keywords_with_none.push(None); // clearing task_state

        let task_state_mutation = (
            prop::sample::select(ids),
            prop::sample::select(keywords_with_none),
        )
            .prop_map(|(id, maybe_kw)| {
                let value = match maybe_kw {
                    Some(kw) => Value::String(kw),
                    None => Value::Null,
                };
                Mutation::Update {
                    entity: "block".to_string(),
                    id,
                    fields: [("task_state".to_string(), value)].into_iter().collect(),
                }
            });

        prop_oneof![3 => content_mutation, 2 => task_state_mutation].boxed()
    } else {
        content_mutation.boxed()
    }
}

/// Generate mutations for render source blocks (change render DSL expression).
///
/// Weighted sampling: the last expression — the `focus_chain()` + `chain_ops()`
/// mobile-bar fixture — drives the value-fn provider invariants
/// (`vfn11/12/13`). A uniform sample over 6 variants hits it only ~17 %
/// of the time, which leaves those invariants observing zero providers
/// in most runs. Weight it at roughly the combined weight of the other
/// expressions so it shows up in at least half of render mutations.
pub fn generate_render_source_mutation(ids: Vec<EntityUri>) -> impl Strategy<Value = Mutation> {
    let expressions = valid_render_expression_strings();
    let last_idx = expressions.len().saturating_sub(1);
    let mut weighted: Vec<String> = Vec::with_capacity(expressions.len() + 4);
    weighted.extend(expressions.iter().take(last_idx).cloned());
    // Replicate the mobile-bar fixture N-1 times so the sampler hits it
    // at parity with the rest of the set combined.
    for _ in 0..last_idx.max(1) {
        weighted.push(expressions[last_idx].clone());
    }

    (prop::sample::select(ids), prop::sample::select(weighted)).prop_map(|(id, expr)| {
        Mutation::Update {
            entity: "block".to_string(),
            id,
            fields: [("content".to_string(), Value::String(expr))]
                .into_iter()
                .collect(),
        }
    })
}

/// Generate mutations for profile source blocks (change entity profile YAML).
pub fn generate_profile_content_mutation(ids: Vec<EntityUri>) -> impl Strategy<Value = Mutation> {
    let yamls: Vec<String> = VALID_PROFILE_YAMLS.iter().map(|s| s.to_string()).collect();
    (prop::sample::select(ids), prop::sample::select(yamls)).prop_map(|(id, yaml)| {
        Mutation::Update {
            entity: "block".to_string(),
            id,
            fields: [("content".to_string(), Value::String(yaml))]
                .into_iter()
                .collect(),
        }
    })
}
