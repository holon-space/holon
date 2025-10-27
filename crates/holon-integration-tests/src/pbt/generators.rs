//! Proptest strategy generators for PBT transitions.

use std::collections::HashSet;
use std::path::Path;

use proptest::prelude::*;

use holon_api::block::Block;
use holon_api::{ContentType, EntityUri, QueryLanguage, SourceLanguage, TaskState, Value};
use holon_orgmode::OrgRenderer;
use holon_orgmode::models::OrgBlockExt;

use super::query::{QueryTable, TestQuery};
use super::reference_state::{VALID_PROFILE_YAMLS, valid_render_expression_strings};
use super::types::Mutation;
use holon_api::predicate::Predicate;

use std::collections::HashMap;

/// A set of TODO keywords generated per test case.
/// Drives both the `#+TODO:` org header and the task_state mutation generator.
#[derive(Debug, Clone)]
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

/// Generate single-line block content for headlines.
/// Headlines must be single-line because the org parser treats newlines in
/// headline text as content boundaries — multi-line headlines cause
/// `:PROPERTIES:` drawers to be embedded in the content.
pub fn content_strategy() -> BoxedStrategy<String> {
    "[A-Z][a-zA-Z0-9 ]{0,20}".prop_map(|s| s).boxed()
}

/// Same as `content_strategy` but for edit mutations (lowercase start).
pub fn edit_content_strategy() -> BoxedStrategy<String> {
    prop_oneof![
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
    .boxed()
}

/// Build the blocks for an index.org heading with a query source + render source,
/// then render them to org text.
fn index_org_content(
    headline: &str,
    id: &str,
    query_lang: QueryLanguage,
    query_source: &str,
    render_expr: &str,
) -> String {
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

    let blocks = vec![heading, query_block, render_entity];
    OrgRenderer::render_entitys(
        &blocks,
        Path::new("/index.org"),
        &EntityUri::block("gen-placeholder"),
    )
}

pub fn generate_org_file_content() -> impl Strategy<Value = (String, String)> {
    generate_org_file_content_with_keywords(None, true)
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
) -> BoxedStrategy<(String, String)> {
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
                "[A-Z][a-z][a-zA-Z0-9 ]{0,19}",
                "[a-z0-9-]+",
                prop::bool::ANY,
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
            let blocks: Vec<Block> = headings
                .into_iter()
                .enumerate()
                .map(|(i, (headline, id, make_task))| {
                    let mut b = Block::new_text(EntityUri::block(&id), doc_uri.clone(), &headline);
                    b.set_property("ID", Value::String(id));
                    // Assign a task keyword to ~50% of headlines when keywords exist.
                    // Cycle through keywords using the index for variety.
                    if make_task && !all_keywords.is_empty() {
                        let kw = &all_keywords[i % all_keywords.len()];
                        b.set_task_state(Some(TaskState::from_keyword(kw)));
                    }
                    b
                })
                .collect();
            let rendered = OrgRenderer::render_entitys(
                &blocks,
                Path::new(&filename),
                &EntityUri::block("gen-placeholder"),
            );
            let content = match &ks {
                Some(set) => format!("{}\n{}", set.to_org_header(), rendered),
                None => rendered,
            };
            (filename, content)
        });

    let index_file_prql = ("[A-Z][a-zA-Z0-9 ]{0,15}", "[a-z0-9-]+").prop_map(|(headline, id)| {
        let content = index_org_content(
            &headline,
            &id,
            QueryLanguage::HolonPrql,
            "from children\n",
            "list(#{item_template: row(state_toggle(col(\"task_state\")), editable_text(col(\"content\")))})",
        );
        ("index.org".to_string(), content)
    });

    let index_file_gql = ("[A-Z][a-zA-Z0-9 ]{0,15}", "[a-z0-9-]+").prop_map(|(headline, id)| {
        let content = index_org_content(
            &headline,
            &id,
            QueryLanguage::HolonGql,
            "MATCH (n) RETURN n\n",
            "list(#{item_template: row(text(\"node\"))})",
        );
        ("index.org".to_string(), content)
    });

    let index_file_gql_varlen =
        ("[A-Z][a-zA-Z0-9 ]{0,15}", "[a-z0-9-]+").prop_map(|(headline, id)| {
            let content = index_org_content(
                &headline,
                &id,
                QueryLanguage::HolonGql,
                "MATCH (root:block)<-[:CHILD_OF*1..3]-(d:block) RETURN d\n",
                "list(#{item_template: row(text(\"varlen\"))})",
            );
            ("index.org".to_string(), content)
        });

    let index_file_sql = ("[A-Z][a-zA-Z0-9 ]{0,15}", "[a-z0-9-]+").prop_map(|(headline, id)| {
        let content = index_org_content(
            &headline,
            &id,
            QueryLanguage::HolonSql,
            "SELECT * FROM nodes\n",
            "list(#{item_template: row(text(\"sql node\"))})",
        );
        ("index.org".to_string(), content)
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
            let content = OrgRenderer::render_entitys(
                &blocks,
                Path::new(&filename),
                &EntityUri::block("gen-placeholder"),
            );
            (filename, content)
        });

    if allow_index_override {
        prop_oneof![
            3 => regular_file,
            2 => index_file_prql,
            1 => index_file_gql,
            1 => index_file_gql_varlen,
            1 => index_file_sql,
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
pub fn generate_query_language() -> impl Strategy<Value = QueryLanguage> {
    prop_oneof![
        5 => Just(QueryLanguage::HolonPrql),
        3 => Just(QueryLanguage::HolonSql),
    ]
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
