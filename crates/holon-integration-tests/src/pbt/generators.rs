//! Proptest strategy generators for PBT transitions.

use std::collections::HashSet;
use std::path::Path;

use proptest::prelude::*;

use holon_api::block::Block;
use holon_api::{ContentType, EntityUri, QueryLanguage, SourceLanguage, TaskState, Value};
use holon_orgmode::OrgRenderer;
use holon_orgmode::models::OrgBlockExt;

use super::query::{QueryTable, TestPredicate, TestQuery};
use super::reference_state::{VALID_PROFILE_YAMLS, VALID_RENDER_EXPRESSIONS};
use super::types::Mutation;

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

/// Build the blocks for an index.org heading with a query source + render source,
/// then render them to org text.
fn index_org_content(
    headline: &str,
    id: &str,
    query_lang: QueryLanguage,
    query_source: &str,
    render_expr: &str,
) -> String {
    let doc_uri = EntityUri::doc("index.org");
    let heading_uri = EntityUri::block(id);

    let mut heading = Block::new_text(
        heading_uri.clone(),
        doc_uri.clone(),
        doc_uri.clone(),
        headline,
    );
    heading.set_property("ID", Value::String(id.to_string()));

    let mut query_block = Block::new_source(
        EntityUri::block(&format!("{id}::src::0")),
        heading_uri.clone(),
        doc_uri.clone(),
        SourceLanguage::Query(query_lang).to_string(),
        query_source,
    );
    query_block.set_sequence(1);

    let mut render_block = Block::new_source(
        EntityUri::block(&format!("{id}::render::0")),
        heading_uri,
        doc_uri,
        "render",
        render_expr,
    );
    render_block.set_sequence(2);

    let blocks = vec![heading, query_block, render_block];
    OrgRenderer::render_blocks(&blocks, Path::new("/index.org"), "index.org")
}

pub fn generate_org_file_content() -> impl Strategy<Value = (String, String)> {
    generate_org_file_content_with_keywords(None)
}

pub fn generate_org_file_content_with_keywords(
    keyword_set: Option<TodoKeywordSet>,
) -> impl Strategy<Value = (String, String)> {
    use proptest::collection::vec as prop_vec;

    let ks = keyword_set.clone();
    let regular_file = (
        "[a-z_]+_[0-9]+\\.org",
        prop_vec(("[A-Z][a-zA-Z0-9 ]{0,20}", "[a-z0-9-]+"), 1..=5),
    )
        .prop_map(move |(filename, headings)| {
            let doc_uri = EntityUri::doc(&filename);
            let blocks: Vec<Block> = headings
                .into_iter()
                .map(|(headline, id)| {
                    let mut b = Block::new_text(
                        EntityUri::block(&id),
                        doc_uri.clone(),
                        doc_uri.clone(),
                        &headline,
                    );
                    b.set_property("ID", Value::String(id));
                    b
                })
                .collect();
            let rendered = OrgRenderer::render_blocks(&blocks, Path::new(&filename), &filename);
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
            "from children\nselect {id, content, parent_id}\n",
            "list item_template:(row (text content:this.content))",
        );
        ("index.org".to_string(), content)
    });

    let index_file_gql = ("[A-Z][a-zA-Z0-9 ]{0,15}", "[a-z0-9-]+").prop_map(|(headline, id)| {
        let content = index_org_content(
            &headline,
            &id,
            QueryLanguage::HolonGql,
            "MATCH (n) RETURN n\n",
            "list item_template:(row (text content:\"node\"))",
        );
        ("index.org".to_string(), content)
    });

    let index_file_gql_varlen =
        ("[A-Z][a-zA-Z0-9 ]{0,15}", "[a-z0-9-]+").prop_map(|(headline, id)| {
            let content = index_org_content(
                &headline,
                &id,
                QueryLanguage::HolonGql,
                "MATCH (root:Block)<-[:CHILD_OF*1..3]-(d:Block) RETURN d.id, d.content\n",
                "list item_template:(row (text content:\"varlen\"))",
            );
            ("index.org".to_string(), content)
        });

    let index_file_sql = ("[A-Z][a-zA-Z0-9 ]{0,15}", "[a-z0-9-]+").prop_map(|(headline, id)| {
        let content = index_org_content(
            &headline,
            &id,
            QueryLanguage::HolonSql,
            "SELECT id FROM nodes\n",
            "list item_template:(row (text content:\"sql node\"))",
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
            let doc_uri = EntityUri::doc(&filename);
            let heading_uri = EntityUri::block(&id);

            let mut heading = Block::new_text(
                heading_uri.clone(),
                doc_uri.clone(),
                doc_uri.clone(),
                &headline,
            );
            heading.set_property("ID", Value::String(id.clone()));

            let mut profile_block = Block::new_source(
                EntityUri::block(&format!("{id}::src::0")),
                heading_uri,
                doc_uri,
                "holon_entity_profile_yaml",
                &*yaml,
            );
            profile_block.set_sequence(1);

            let blocks = vec![heading, profile_block];
            let content = OrgRenderer::render_blocks(&blocks, Path::new(&filename), &filename);
            (filename, content)
        });

    prop_oneof![
        3 => regular_file,
        2 => index_file_prql,
        1 => index_file_gql,
        1 => index_file_gql_varlen,
        1 => index_file_sql,
        1 => file_with_profile,
    ]
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
    existing_block_ids: Vec<String>,
    text_block_ids: Vec<String>,
    doc_uris: Vec<String>,
    no_content_update_ids: HashSet<String>,
) -> impl Strategy<Value = Mutation> {
    let mut valid_parent_ids_for_text = doc_uris.clone();
    valid_parent_ids_for_text.extend(existing_block_ids.iter().cloned());

    let valid_parent_ids_for_source = text_block_ids;

    let create_text = (
        "[a-zA-Z][a-zA-Z0-9 ]{0,20}",
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
                id: format!("block:block-{}", next_id),
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
                id: format!("block:block-{}", next_id),
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

    let updatable_content_ids: Vec<String> = ids
        .iter()
        .filter(|id| !no_content_update_ids.contains(id.as_str()))
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
            "[a-zA-Z][a-zA-Z0-9 ]{0,20}",
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
    let prop_target_ids: Vec<String> = ids
        .iter()
        .filter(|id| !no_content_update_ids.contains(id.as_str()))
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

pub fn generate_predicate() -> impl Strategy<Value = TestPredicate> {
    prop_oneof![
        Just(TestPredicate::Neq(
            "content".into(),
            Value::String("".into())
        )),
        Just(TestPredicate::Eq(
            "content_type".into(),
            Value::String("text".into())
        )),
        Just(TestPredicate::Eq(
            "content_type".into(),
            Value::String("source".into())
        )),
        Just(TestPredicate::IsNotNull("source_language".into())),
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
    ids: Vec<String>,
    keyword_set: Option<TodoKeywordSet>,
) -> impl Strategy<Value = Mutation> {
    let content_mutation =
        (prop::sample::select(ids.clone()), "[A-Z][a-zA-Z0-9 ]{0,20}").prop_map(|(id, content)| {
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
pub fn generate_render_source_mutation(ids: Vec<String>) -> impl Strategy<Value = Mutation> {
    let expressions: Vec<String> = VALID_RENDER_EXPRESSIONS
        .iter()
        .map(|s| s.to_string())
        .collect();

    (prop::sample::select(ids), prop::sample::select(expressions)).prop_map(|(id, expr)| {
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
pub fn generate_profile_content_mutation(ids: Vec<String>) -> impl Strategy<Value = Mutation> {
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
