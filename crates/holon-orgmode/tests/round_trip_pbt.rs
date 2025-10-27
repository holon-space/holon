//! Comprehensive Property-Based Tests for org-mode round-tripping.
//!
//! Uses a simple normalize-and-compare approach:
//! 1. Generate valid Block hierarchies
//! 2. Serialize to Org format
//! 3. Parse back
//! 4. Normalize both and compare for equality

use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::types::Timestamp;
use holon_api::Value;
use holon_api::{ContentType, Priority, SourceLanguage, Tags, TaskState};
use holon_filesystem::directory::ROOT_ID;
use holon_orgmode::models::{
    OrgBlockExt, OrgDocumentExt, ToOrg, DEFAULT_ACTIVE_KEYWORDS, DEFAULT_DONE_KEYWORDS,
};
use holon_orgmode::org_renderer::OrgRenderer;
use holon_orgmode::parser::parse_org_file;
use holon_orgmode::Document;
use proptest::prelude::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use uuid::Uuid;

// ============================================================================
// Normalized representation for comparison
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedBlock {
    id: EntityUri,
    parent_id: EntityUri,
    content_type: ContentType,
    // Headline fields
    level: i64,
    title: String,
    task_state: Option<TaskState>,
    priority: Option<holon_api::Priority>,
    tags: BTreeSet<String>,
    // Source block fields
    source_language: Option<String>,
    source_name: Option<String>,
    header_args: BTreeMap<String, String>,
    // Planning timestamps
    scheduled: Option<String>,
    deadline: Option<String>,
    // Custom drawer properties (non-internal keys like column-order, collapse-to, etc.)
    drawer_properties: BTreeMap<String, String>,
    // Ordering
    sequence: i64,
}

impl NormalizedBlock {
    fn from_block(block: &Block) -> Self {
        let title = block.org_title().trim().to_string();

        let tags: BTreeSet<String> = block.tags().to_set();

        let header_args: BTreeMap<String, String> = block
            .get_source_header_args()
            .into_iter()
            .filter(|(k, _)| k != "id") // Skip 'id' as it's auto-added
            .map(|(k, v)| (k, v.as_string().unwrap_or_default().to_string()))
            .collect();

        let drawer_properties: BTreeMap<String, String> =
            block.drawer_properties().into_iter().collect();

        NormalizedBlock {
            id: block.id.clone(),
            parent_id: block.parent_id.clone(),
            content_type: block.content_type,
            level: block.level(),
            title,
            task_state: block.task_state(),
            priority: block.priority(),
            tags,
            scheduled: block.scheduled().map(|t| t.to_string()),
            deadline: block.deadline().map(|t| t.to_string()),
            source_language: block.source_language.as_ref().map(|l| l.to_string()),
            source_name: block.source_name.clone(),
            header_args,
            drawer_properties,
            sequence: block.sequence(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedDocument {
    title: Option<String>,
    blocks: Vec<NormalizedBlock>,
}

impl NormalizedDocument {
    fn from_doc_and_blocks(doc: &Document, blocks: &[Block]) -> Self {
        let mut normalized_blocks: Vec<NormalizedBlock> =
            blocks.iter().map(NormalizedBlock::from_block).collect();
        normalized_blocks.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));

        NormalizedDocument {
            title: doc.org_title().map(|t| t.trim().to_string()),
            blocks: normalized_blocks,
        }
    }
}

// ============================================================================
// Strategy: Valid identifiers and text
// ============================================================================

fn valid_identifier() -> impl Strategy<Value = String> {
    "[a-zA-Z][a-zA-Z0-9_]{0,19}[a-zA-Z0-9]?"
}

fn valid_tag() -> impl Strategy<Value = String> {
    "[a-zA-Z][a-zA-Z0-9_]{0,14}"
}

fn valid_title() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9][a-zA-Z0-9 ]{0,48}[a-zA-Z0-9]"
}

fn valid_body() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 .,!?\n]{10,200}"
}

fn valid_source_code() -> impl Strategy<Value = String> {
    // Must start with non-whitespace (orgize strips leading blank lines from source content)
    "[a-zA-Z0-9_=(){}\\[\\];,.][a-zA-Z0-9_ =(){}\\[\\];,.\n]{9,99}"
}

fn valid_property_value() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_-]{1,30}"
}

fn valid_timestamp() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("<2024-01-15 Mon>".to_string()),
        Just("<2024-06-20 Thu 14:00>".to_string()),
        Just("<2024-12-31 Tue 09:30>".to_string()),
    ]
}

// ============================================================================
// Strategy: Document
// ============================================================================

fn document_strategy() -> impl Strategy<Value = Document> {
    (
        prop::option::of(valid_title()),
        prop::option::of(prop_oneof![
            Just(vec![
                TaskState::active("TODO"),
                TaskState::active("INPROGRESS"),
                TaskState::done("DONE"),
                TaskState::done("CANCELLED"),
            ]),
            Just(vec![TaskState::active("TODO"), TaskState::done("DONE")]),
            Just(vec![
                TaskState::active("TASK"),
                TaskState::active("WORK"),
                TaskState::done("COMPLETE"),
            ]),
        ]),
    )
        .prop_map(|(title, todo_keywords)| {
            let id = EntityUri::doc(&format!("test-{}", Uuid::new_v4()));
            let mut doc = Document::new(
                id.to_string(),
                EntityUri::doc_root().to_string(),
                "test.org".to_string(),
            );
            doc.set_org_title(title);
            doc.set_todo_keywords(todo_keywords);
            doc
        })
}

// ============================================================================
// Strategy: Properties drawer (with explicit :ID:)
// ============================================================================

#[derive(Debug, Clone)]
struct PropertiesDrawer {
    /// Explicit :ID: in the drawer. None means "no :ID: in the org properties drawer"
    /// — the renderer will inject one from block.id.
    explicit_id: Option<String>,
    other_props: HashMap<String, String>,
}

fn properties_drawer_strategy() -> impl Strategy<Value = PropertiesDrawer> {
    (
        // ~70% of headlines get an explicit :ID:, ~30% don't (simulates user-created headlines)
        prop::option::weighted(
            0.7,
            "[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}",
        ),
        prop::option::of(prop::collection::hash_map(
            prop_oneof![
                Just("VIEW".to_string()),
                Just("REGION".to_string()),
                Just("CUSTOM".to_string()),
                Just("column-order".to_string()),
                Just("collapse-to".to_string()),
                Just("ideal-width".to_string()),
                Just("column-priority".to_string()),
            ],
            valid_property_value(),
            1..=3,
        )),
    )
        .prop_map(|(explicit_id, other_props)| PropertiesDrawer {
            explicit_id,
            other_props: other_props.unwrap_or_default(),
        })
}

// ============================================================================
// Strategy: Source Block
// ============================================================================

#[derive(Debug, Clone)]
struct SourceBlockSpec {
    id: EntityUri,
    language: String,
    source: String,
    name: Option<String>,
    header_args: HashMap<String, String>,
    custom_properties: HashMap<String, String>,
}

fn source_block_spec_strategy() -> impl Strategy<Value = SourceBlockSpec> {
    (
        prop_oneof![
            Just("holon_prql".to_string()),
            Just("python".to_string()),
            Just("rust".to_string()),
            Just("holon_sql".to_string()),
        ],
        valid_source_code(),
        prop::option::of(valid_identifier()),
        prop::collection::hash_map(
            prop_oneof![
                Just("results".to_string()),
                Just("session".to_string()),
                Just("connection".to_string()),
            ],
            valid_identifier(),
            0..=2,
        ),
        prop::collection::hash_map(
            prop_oneof![
                Just("column-order".to_string()),
                Just("collapse-to".to_string()),
                Just("column-priority".to_string()),
            ],
            valid_property_value(),
            0..=2,
        ),
    )
        .prop_map(
            |(language, source, name, header_args, custom_properties)| SourceBlockSpec {
                id: EntityUri::block(&Uuid::new_v4().to_string()),
                language,
                source,
                name,
                header_args,
                custom_properties,
            },
        )
}

// ============================================================================
// Strategy: Headline Block
// ============================================================================

#[derive(Debug, Clone)]
struct HeadlineSpec {
    /// Internal block ID. Used as Block.id.
    block_id: EntityUri,
    properties_drawer: PropertiesDrawer,
    level: i64,
    task_state: Option<TaskState>,
    priority: Option<holon_api::Priority>,
    title: String,
    tags: Option<Vec<String>>,
    body: Option<String>,
    scheduled: Option<String>,
    deadline: Option<String>,
    source_blocks: Vec<SourceBlockSpec>,
    child_headlines: Vec<HeadlineSpec>,
}

impl HeadlineSpec {
    fn id(&self) -> &EntityUri {
        &self.block_id
    }

    fn to_block(
        &self,
        parent_id: &EntityUri,
        document_id: &EntityUri,
        sequence: &mut i64,
    ) -> Vec<Block> {
        let mut blocks = Vec::new();

        let content = match &self.body {
            Some(b) => format!("{}\n{}", self.title, b),
            None => self.title.clone(),
        };

        let mut block = Block::new_text(
            self.id().clone(),
            parent_id.clone(),
            document_id.clone(),
            &content,
        );
        block.set_level(self.level);
        block.set_sequence(*sequence);
        *sequence += 1;

        block.set_task_state(self.task_state.clone());
        block.set_priority(self.priority);

        if let Some(ref tags) = self.tags {
            if !tags.is_empty() {
                block.set_tags(holon_api::Tags::from(tags.clone()));
            }
        }

        block.set_scheduled(
            self.scheduled
                .as_deref()
                .and_then(|s| holon_api::types::Timestamp::parse(s).ok()),
        );
        block.set_deadline(
            self.deadline
                .as_deref()
                .and_then(|s| holon_api::types::Timestamp::parse(s).ok()),
        );

        // Set org properties as flat keys (only include :ID: if explicitly set in drawer)
        if let Some(ref explicit_id) = self.properties_drawer.explicit_id {
            block.set_property("ID", holon_api::Value::String(explicit_id.clone()));
        }
        for (k, v) in &self.properties_drawer.other_props {
            block.set_property(k, holon_api::Value::String(v.clone()));
        }

        // Children relationship is established via parent_id
        blocks.push(block);

        // Create source block entities
        for sb_spec in &self.source_blocks {
            let header_args: HashMap<String, Value> = sb_spec
                .header_args
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect();

            let mut src_block = Block {
                id: sb_spec.id.clone(),
                parent_id: self.id().clone(),
                document_id: document_id.clone(),
                content: sb_spec.source.clone(),
                content_type: ContentType::Source,
                source_language: Some(sb_spec.language.parse::<SourceLanguage>().unwrap()),
                source_name: sb_spec.name.clone(),
                properties: HashMap::new(),
                created_at: chrono::Utc::now().timestamp_millis(),
                updated_at: chrono::Utc::now().timestamp_millis(),
            };
            if !sb_spec.header_args.is_empty() {
                src_block.set_source_header_args(header_args);
            }
            for (k, v) in &sb_spec.custom_properties {
                src_block.set_property(k, Value::String(v.clone()));
            }
            src_block.set_sequence(*sequence);
            *sequence += 1;
            blocks.push(src_block);
        }

        // Recursively create child headline blocks
        for child in &self.child_headlines {
            blocks.extend(child.to_block(self.id(), document_id, sequence));
        }

        blocks
    }
}

fn headline_spec_strategy(
    level: i64,
    max_children: usize,
    max_depth: usize,
) -> impl Strategy<Value = HeadlineSpec> {
    (
        properties_drawer_strategy(),
        prop::option::of(prop_oneof![
            Just(TaskState::active("TODO")),
            Just(TaskState::done("DONE")),
            Just(TaskState::active("DOING")),
            Just(TaskState::done("CANCELLED")),
            Just(TaskState::done("CLOSED")),
        ]),
        prop::option::of(prop_oneof![
            Just(holon_api::Priority::Low),
            Just(holon_api::Priority::Medium),
            Just(holon_api::Priority::High),
        ]),
        valid_title(),
        prop::option::of(prop::collection::vec(valid_tag(), 1..=3)),
        prop::option::of(valid_body()),
        prop::option::of(valid_timestamp()),
        prop::option::of(valid_timestamp()),
        prop::collection::vec(source_block_spec_strategy(), 0..=3),
    )
        .prop_flat_map(
            move |(
                props,
                task_state,
                priority,
                title,
                tags,
                body,
                scheduled,
                deadline,
                source_blocks,
            )| {
                // Use explicit_id from drawer if present, otherwise generate a fresh UUID
                let raw_id = props
                    .explicit_id
                    .clone()
                    .unwrap_or_else(|| Uuid::new_v4().to_string());
                let block_id = EntityUri::block(&raw_id);

                let headline = HeadlineSpec {
                    block_id,
                    properties_drawer: props,
                    level,
                    task_state,
                    priority,
                    title,
                    tags,
                    body,
                    scheduled,
                    deadline,
                    source_blocks,
                    child_headlines: Vec::new(),
                };

                if max_depth == 0 || max_children == 0 {
                    Just(headline).boxed()
                } else {
                    let child_level = level + 1;
                    let child_max_children = max_children.saturating_sub(1);
                    let child_max_depth = max_depth - 1;

                    prop::collection::vec(
                        headline_spec_strategy(child_level, child_max_children, child_max_depth),
                        0..=max_children,
                    )
                    .prop_map(move |children| {
                        let mut h = headline.clone();
                        h.child_headlines = children;
                        h
                    })
                    .boxed()
                }
            },
        )
}

// ============================================================================
// Strategy: Complete Document
// ============================================================================

#[derive(Debug, Clone)]
struct CompleteDocument {
    document: Document,
    root_headlines: Vec<HeadlineSpec>,
}

impl CompleteDocument {
    fn all_blocks(&self) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut sequence = 0i64;
        let doc_id = EntityUri::from_raw(self.document.id.as_str());

        for headline in &self.root_headlines {
            blocks.extend(headline.to_block(&doc_id, &doc_id, &mut sequence));
        }

        blocks
    }

    fn ensure_todo_keywords_configured(&mut self) {
        use holon_orgmode::models::{DEFAULT_ACTIVE_KEYWORDS, DEFAULT_DONE_KEYWORDS};

        let mut all_todos = std::collections::HashSet::new();

        fn collect_todos(headline: &HeadlineSpec, todos: &mut std::collections::HashSet<String>) {
            if let Some(ref todo) = headline.task_state {
                todos.insert(todo.to_string());
            }
            for child in &headline.child_headlines {
                collect_todos(child, todos);
            }
        }

        for headline in &self.root_headlines {
            collect_todos(headline, &mut all_todos);
        }

        // Only emit #+TODO: if any keyword falls outside the parser defaults.
        // This ensures the PBT exercises the no-config code path when possible.
        let all_within_defaults = all_todos.iter().all(|kw| {
            DEFAULT_ACTIVE_KEYWORDS.contains(&kw.as_str())
                || DEFAULT_DONE_KEYWORDS.contains(&kw.as_str())
        });

        if all_within_defaults {
            self.document.set_todo_keywords(None);
        } else {
            let states: Vec<TaskState> = all_todos
                .iter()
                .map(|kw| {
                    if DEFAULT_DONE_KEYWORDS.contains(&kw.as_str()) {
                        TaskState::done(kw)
                    } else {
                        TaskState::active(kw)
                    }
                })
                .collect();
            self.document.set_todo_keywords(Some(states));
        }
    }
}

fn complete_document_strategy() -> impl Strategy<Value = CompleteDocument> {
    prop_oneof![
        // ~80% random documents
        4 => document_strategy().prop_flat_map(|doc| {
            let doc_clone = doc.clone();
            prop::collection::vec(headline_spec_strategy(1, 2, 2), 1..=4).prop_map(
                move |root_headlines| {
                    let mut cd = CompleteDocument {
                        document: doc_clone.clone(),
                        root_headlines,
                    };
                    cd.ensure_todo_keywords_configured();
                    cd
                },
            )
        }),
        // ~20% corpus-seeded from assets/default/index.org
        1 => Just(()).prop_map(|_| corpus_document_from_index_org()),
    ]
}

fn corpus_document_from_index_org() -> CompleteDocument {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = std::path::Path::new(manifest_dir).join("../../assets/default/index.org");
    let org_text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));

    let root = PathBuf::from("/test");
    let file_path = PathBuf::from("/test/index.org");
    let parse_result = parse_org_file(&file_path, &org_text, ROOT_ID, 0, &root)
        .unwrap_or_else(|e| panic!("Failed to parse index.org: {e}"));

    let doc = parse_result.document;
    let blocks = parse_result.blocks;

    let root_headlines = blocks
        .iter()
        .filter(|b| b.parent_id.is_doc() && b.content_type == ContentType::Text)
        .map(|root_block| block_to_headline_spec(root_block, &blocks))
        .collect();

    CompleteDocument {
        document: doc,
        root_headlines,
    }
}

fn block_to_headline_spec(block: &Block, all_blocks: &[Block]) -> HeadlineSpec {
    let (title, body) = {
        let content = &block.content;
        match content.find('\n') {
            Some(pos) => (
                content[..pos].to_string(),
                Some(content[pos + 1..].to_string()),
            ),
            None => (content.clone(), None),
        }
    };

    let explicit_id = block
        .properties
        .get("ID")
        .and_then(|v| v.as_string())
        .map(|s| s.to_string());

    let other_props: HashMap<String, String> = block.drawer_properties().into_iter().collect();

    let source_blocks: Vec<SourceBlockSpec> = all_blocks
        .iter()
        .filter(|b| b.parent_id == block.id && b.content_type == ContentType::Source)
        .map(|sb| {
            let header_args: HashMap<String, String> = sb
                .get_source_header_args()
                .into_iter()
                .filter(|(k, _)| k != "id")
                .map(|(k, v)| (k, v.as_string().unwrap_or_default().to_string()))
                .collect();
            let custom_properties: HashMap<String, String> =
                sb.drawer_properties().into_iter().collect();
            SourceBlockSpec {
                id: sb.id.clone(),
                language: sb
                    .source_language
                    .as_ref()
                    .map(|l| l.to_string())
                    .unwrap_or_default(),
                source: sb.content.clone(),
                name: sb.source_name.clone(),
                header_args,
                custom_properties,
            }
        })
        .collect();

    let child_headlines: Vec<HeadlineSpec> = all_blocks
        .iter()
        .filter(|b| b.parent_id == block.id && b.content_type == ContentType::Text)
        .map(|child| block_to_headline_spec(child, all_blocks))
        .collect();

    HeadlineSpec {
        block_id: block.id.clone(),
        properties_drawer: PropertiesDrawer {
            explicit_id,
            other_props,
        },
        level: block.level(),
        task_state: block.task_state(),
        priority: block.priority(),
        title,
        tags: {
            let tags = block.tags();
            if tags.as_slice().is_empty() {
                None
            } else {
                Some(tags.as_slice().to_vec())
            }
        },
        body,
        scheduled: block.scheduled().map(|t| t.to_string()),
        deadline: block.deadline().map(|t| t.to_string()),
        source_blocks,
        child_headlines,
    }
}

// ============================================================================
// Helpers
// ============================================================================

fn build_org_text(doc: &Document, blocks: &[Block]) -> String {
    let file_path = PathBuf::from("/test/test.org");
    let file_id = doc.id.as_str();

    let mut org_text = doc.to_org();
    if !org_text.is_empty() && !org_text.ends_with('\n') {
        org_text.push('\n');
    }

    let rendered = OrgRenderer::render_blocks(blocks, &file_path, file_id);
    org_text.push_str(&rendered);

    org_text
}

/// Recursively collect explicit :ID: values from headline specs.
fn collect_explicit_ids(headline: &HeadlineSpec) -> Vec<String> {
    let mut ids = Vec::new();
    if let Some(ref id) = headline.properties_drawer.explicit_id {
        ids.push(id.clone());
    }
    for child in &headline.child_headlines {
        ids.extend(collect_explicit_ids(child));
    }
    ids
}

fn parse_org(org_text: &str) -> Result<holon_orgmode::parser::ParseResult, String> {
    let path = PathBuf::from("/test/test.org");
    let root = PathBuf::from("/test");
    parse_org_file(&path, org_text, ROOT_ID, 0, &root).map_err(|e| e.to_string())
}

// ============================================================================
// PBT: Comprehensive round-trip test
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 100,
        ..ProptestConfig::default()
    })]

    #[test]
    fn test_round_trip(mut complete_doc in complete_document_strategy()) {
        complete_doc.ensure_todo_keywords_configured();

        let blocks = complete_doc.all_blocks();
        let org_text = build_org_text(&complete_doc.document, &blocks);

        let parse_result = match parse_org(&org_text) {
            Ok(r) => r,
            Err(e) => {
                prop_assert!(false, "Parsing failed: {}\n\nOrg text:\n{}", e, org_text);
                return Ok(());
            }
        };

        let expected = NormalizedDocument::from_doc_and_blocks(&complete_doc.document, &blocks);
        let actual = NormalizedDocument::from_doc_and_blocks(&parse_result.document, &parse_result.blocks);

        // Compare document title
        prop_assert_eq!(
            &expected.title,
            &actual.title,
            "Document titles must match"
        );

        // Compare block counts
        prop_assert_eq!(
            expected.blocks.len(),
            actual.blocks.len(),
            "Block counts must match"
        );

        // Compare each block by ID
        for expected_block in &expected.blocks {
            let actual_block = actual.blocks.iter().find(|b| b.id == expected_block.id);

            prop_assert!(
                actual_block.is_some(),
                "Block with ID '{}' must exist after round-trip",
                expected_block.id
            );

            let actual_block = actual_block.unwrap();

            // Compare fields
            // For root-level blocks, parent is the document which has different ID formats
            if expected_block.parent_id.is_doc() || actual_block.parent_id.is_doc() {
                prop_assert_eq!(
                    expected_block.parent_id.is_doc(),
                    actual_block.parent_id.is_doc(),
                    "Root status must match for block '{}'",
                    expected_block.id
                );
            } else {
                prop_assert_eq!(
                    &expected_block.parent_id,
                    &actual_block.parent_id,
                    "Parent ID must match for block '{}'",
                    expected_block.id
                );
            }

            prop_assert_eq!(
                &expected_block.content_type,
                &actual_block.content_type,
                "Content type must match for block '{}'",
                expected_block.id
            );

            if expected_block.content_type == ContentType::Text {
                prop_assert_eq!(
                    expected_block.level,
                    actual_block.level,
                    "Level must match for headline '{}'",
                    expected_block.id
                );

                prop_assert_eq!(
                    &expected_block.task_state,
                    &actual_block.task_state,
                    "Task state must match for headline '{}'",
                    expected_block.id
                );

                prop_assert_eq!(
                    expected_block.priority,
                    actual_block.priority,
                    "Priority must match for headline '{}'",
                    expected_block.id
                );

                prop_assert_eq!(
                    &expected_block.tags,
                    &actual_block.tags,
                    "Tags must match for headline '{}'",
                    expected_block.id
                );

                prop_assert_eq!(
                    &expected_block.drawer_properties,
                    &actual_block.drawer_properties,
                    "Drawer properties must match for headline '{}'",
                    expected_block.id
                );
            }

            if expected_block.content_type == ContentType::Source {
                prop_assert_eq!(
                    &expected_block.source_language,
                    &actual_block.source_language,
                    "Source language must match for source block '{}'",
                    expected_block.id
                );

                prop_assert_eq!(
                    &expected_block.source_name,
                    &actual_block.source_name,
                    "Source name must match for source block '{}'",
                    expected_block.id
                );

                prop_assert_eq!(
                    &expected_block.header_args,
                    &actual_block.header_args,
                    "Header args must match for source block '{}'",
                    expected_block.id
                );

                prop_assert_eq!(
                    &expected_block.drawer_properties,
                    &actual_block.drawer_properties,
                    "Custom properties must match for source block '{}'",
                    expected_block.id
                );
            }
        }

        // Every text block after render→parse must have a non-empty ID
        // (the renderer injects :ID: from block.id, the parser reads it back)
        for parsed_block in &parse_result.blocks {
            if parsed_block.content_type == ContentType::Text {
                prop_assert!(
                    !parsed_block.id.as_str().is_empty(),
                    "Text block must have non-empty ID after render→parse"
                );
            }
        }

        // Explicit :ID: values from the original must be preserved as block.id
        let explicit_ids: std::collections::HashSet<String> = complete_doc
            .root_headlines
            .iter()
            .flat_map(|h| collect_explicit_ids(h))
            .collect();
        for explicit_id in &explicit_ids {
            let found = parse_result
                .blocks
                .iter()
                .any(|b| b.id.as_str() == explicit_id || b.id.id() == explicit_id);
            prop_assert!(
                found,
                "Explicit :ID: '{}' must be preserved after render→parse",
                explicit_id
            );
        }

        // Block ordering: sibling sequences must be strictly increasing,
        // and source blocks must have lower sequences than text siblings
        let mut siblings_by_parent: HashMap<EntityUri, Vec<&Block>> = HashMap::new();
        for block in &parse_result.blocks {
            siblings_by_parent
                .entry(block.parent_id.clone())
                .or_default()
                .push(block);
        }
        for (parent_id, mut siblings) in siblings_by_parent {
            siblings.sort_by_key(|b| b.sequence());
            for window in siblings.windows(2) {
                prop_assert!(
                    window[0].sequence() < window[1].sequence(),
                    "Sibling sequences must be strictly increasing under parent '{}': \
                     '{}' (seq {}) vs '{}' (seq {})",
                    parent_id,
                    window[0].id, window[0].sequence(),
                    window[1].id, window[1].sequence(),
                );
            }

            let source_seqs: Vec<i64> = siblings.iter()
                .filter(|b| b.content_type == ContentType::Source)
                .map(|b| b.sequence())
                .collect();
            let text_seqs: Vec<i64> = siblings.iter()
                .filter(|b| b.content_type == ContentType::Text)
                .map(|b| b.sequence())
                .collect();
            if let (Some(&max_src), Some(&min_txt)) = (source_seqs.iter().max(), text_seqs.iter().min()) {
                prop_assert!(
                    max_src < min_txt,
                    "Source blocks must be ordered before text siblings under parent '{}'",
                    parent_id,
                );
            }
        }
    }

    /// Test string-level render stability: render → parse → render must produce
    /// identical org text. This is stricter than normalized comparison — it catches
    /// whitespace and formatting drift.
    #[test]
    fn test_render_string_stability(mut complete_doc in complete_document_strategy()) {
        complete_doc.ensure_todo_keywords_configured();

        let blocks = complete_doc.all_blocks();
        let org_text_1 = build_org_text(&complete_doc.document, &blocks);

        let parse_result = match parse_org(&org_text_1) {
            Ok(r) => r,
            Err(_) => return Ok(()),
        };

        let org_text_2 = build_org_text(&parse_result.document, &parse_result.blocks);

        prop_assert_eq!(
            &org_text_1,
            &org_text_2,
            "\n=== FIRST RENDER ===\n{}\n=== SECOND RENDER ===\n{}",
            org_text_1,
            org_text_2,
        );
    }

    /// Simulates blocks arriving from Loro in arbitrary order.
    /// The renderer must produce identical output regardless of input block order.
    #[test]
    fn test_render_order_independent(mut complete_doc in complete_document_strategy(), seed in any::<u64>()) {
        complete_doc.ensure_todo_keywords_configured();

        let blocks = complete_doc.all_blocks();
        let file_path = PathBuf::from("/test/test.org");
        let file_id = complete_doc.document.id.as_str();

        let render_canonical = OrgRenderer::render_blocks(&blocks, &file_path, file_id);

        // Deterministic Fisher-Yates shuffle
        let mut shuffled = blocks.clone();
        let len = shuffled.len();
        let mut s = seed;
        for i in (1..len).rev() {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let j = (s >> 33) as usize % (i + 1);
            shuffled.swap(i, j);
        }

        let render_shuffled = OrgRenderer::render_blocks(&shuffled, &file_path, file_id);

        prop_assert_eq!(
            &render_canonical,
            &render_shuffled,
            "\n=== CANONICAL ===\n{}\n=== SHUFFLED ===\n{}",
            render_canonical,
            render_shuffled,
        );
    }

}

// ============================================================================
// Phase 1: Mutation PBTs
// ============================================================================

// -- BlockMutation: in-memory mutations applied to blocks --------------------

#[derive(Debug, Clone)]
enum BlockMutation {
    SetTitle(String),
    SetBody(Option<String>),
    SetTaskState(Option<TaskState>),
    SetPriority(Option<Priority>),
    SetTags(Tags),
    AddTag(String),
    RemoveAllTags,
    SetScheduled(Option<Timestamp>),
    SetDeadline(Option<Timestamp>),
    SetDrawerProperty { key: String, value: String },
    SwapSiblingOrder,
}

fn block_mutation_strategy() -> impl Strategy<Value = BlockMutation> {
    prop_oneof![
        valid_title().prop_map(BlockMutation::SetTitle),
        prop::option::of(valid_body()).prop_map(BlockMutation::SetBody),
        prop::option::of(prop_oneof![
            Just(TaskState::active("TODO")),
            Just(TaskState::done("DONE")),
            Just(TaskState::active("DOING")),
            Just(TaskState::done("CANCELLED")),
            Just(TaskState::done("CLOSED")),
        ])
        .prop_map(BlockMutation::SetTaskState),
        prop::option::of(prop_oneof![
            Just(Priority::Low),
            Just(Priority::Medium),
            Just(Priority::High),
        ])
        .prop_map(BlockMutation::SetPriority),
        prop::collection::vec(valid_tag(), 0..=3)
            .prop_map(|v| BlockMutation::SetTags(Tags::from(v))),
        valid_tag().prop_map(BlockMutation::AddTag),
        Just(BlockMutation::RemoveAllTags),
        valid_timestamp().prop_map(|s| BlockMutation::SetScheduled(Timestamp::parse(&s).ok())),
        valid_timestamp().prop_map(|s| BlockMutation::SetDeadline(Timestamp::parse(&s).ok())),
        (
            prop_oneof![
                Just("VIEW".to_string()),
                Just("REGION".to_string()),
                Just("CUSTOM".to_string()),
                Just("column-order".to_string()),
            ],
            valid_property_value(),
        )
            .prop_map(|(key, value)| BlockMutation::SetDrawerProperty { key, value }),
        Just(BlockMutation::SwapSiblingOrder),
    ]
}

fn apply_mutation(block: &mut Block, mutation: &BlockMutation) {
    match mutation {
        BlockMutation::SetTitle(new_title) => {
            let body = block.body();
            block.set_title_and_body(new_title.clone(), body);
        }
        BlockMutation::SetBody(new_body) => {
            let title = block.org_title().to_string();
            block.set_title_and_body(title, new_body.clone());
        }
        BlockMutation::SetTaskState(state) => {
            block.set_task_state(state.clone());
        }
        BlockMutation::SetPriority(priority) => {
            block.set_priority(*priority);
        }
        BlockMutation::SetTags(tags) => {
            block.set_tags(tags.clone());
        }
        BlockMutation::AddTag(tag) => {
            let mut current = block.tags().as_slice().to_vec();
            current.push(tag.clone());
            block.set_tags(Tags::from(current));
        }
        BlockMutation::RemoveAllTags => {
            block.set_tags(Tags::default());
        }
        BlockMutation::SetScheduled(ts) => {
            block.set_scheduled(ts.clone());
        }
        BlockMutation::SetDeadline(ts) => {
            block.set_deadline(ts.clone());
        }
        BlockMutation::SetDrawerProperty { key, value } => {
            block.set_property(key, Value::String(value.clone()));
            // Also update org_properties JSON to include this property
            let mut drawer = block.drawer_properties();
            drawer.insert(key.clone(), value.clone());
            let mut org_map = serde_json::Map::new();
            let id_val = block
                .get_property("ID")
                .and_then(|v| v.as_string().map(|s| s.to_string()));
            if let Some(id_str) = id_val {
                org_map.insert("ID".to_string(), serde_json::Value::String(id_str));
            }
            for (k, v) in &drawer {
                org_map.insert(k.clone(), serde_json::Value::String(v.clone()));
            }
            block.set_org_properties(Some(serde_json::to_string(&org_map).unwrap()));
        }
        BlockMutation::SwapSiblingOrder => {
            // Handled at the block-list level, not on individual blocks
        }
    }
}

/// Find two text siblings that can be swapped under the same parent.
/// Returns (index_a, index_b) into the blocks vec.
fn find_swappable_text_siblings(blocks: &[Block]) -> Option<(usize, usize)> {
    // Group text blocks by parent_id
    let mut by_parent: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, b) in blocks.iter().enumerate() {
        if b.content_type == ContentType::Text {
            by_parent.entry(b.parent_id.as_str()).or_default().push(i);
        }
    }
    // Find first parent with 2+ text children
    for indices in by_parent.values() {
        if indices.len() >= 2 {
            return Some((indices[0], indices[1]));
        }
    }
    None
}

/// Reassign sequences to blocks in the order the renderer would output them.
///
/// The renderer sorts: root blocks by (sequence, id), children by (content_type [source=0, text=1], sequence, id).
/// The parser then assigns monotonic sequences in document order.
/// This function replicates that assignment so expected state matches actual parsed state.
fn reassign_parser_sequences(blocks: &mut [Block]) {
    // Build parent→children mapping
    let ids: Vec<(EntityUri, EntityUri, ContentType, i64, String)> = blocks
        .iter()
        .map(|b| {
            (
                b.id.clone(),
                b.parent_id.clone(),
                b.content_type,
                b.sequence(),
                b.id.as_str().to_string(),
            )
        })
        .collect();

    // Compute render order (DFS following renderer's sort)
    let mut render_order: Vec<usize> = Vec::new();

    fn visit(
        parent_id: &str,
        ids: &[(EntityUri, EntityUri, ContentType, i64, String)],
        render_order: &mut Vec<usize>,
    ) {
        let mut children: Vec<usize> = ids
            .iter()
            .enumerate()
            .filter(|(_, (_, pid, _, _, _))| pid.as_str() == parent_id)
            .map(|(i, _)| i)
            .collect();

        // Sort like the renderer: source before text, then by sequence, then by id
        children.sort_by(|&a, &b| {
            let a_type: i64 = if ids[a].2 == ContentType::Source {
                0
            } else {
                1
            };
            let b_type: i64 = if ids[b].2 == ContentType::Source {
                0
            } else {
                1
            };
            a_type
                .cmp(&b_type)
                .then_with(|| ids[a].3.cmp(&ids[b].3))
                .then_with(|| ids[a].4.cmp(&ids[b].4))
        });

        for &idx in &children {
            render_order.push(idx);
            visit(ids[idx].0.as_str(), ids, render_order);
        }
    }

    // Find root blocks (parent is a document URI)
    let mut roots: Vec<usize> = ids
        .iter()
        .enumerate()
        .filter(|(_, (_, pid, _, _, _))| pid.is_document())
        .map(|(i, _)| i)
        .collect();
    roots.sort_by(|&a, &b| {
        ids[a]
            .3
            .cmp(&ids[b].3)
            .then_with(|| ids[a].4.cmp(&ids[b].4))
    });

    for &root_idx in &roots {
        render_order.push(root_idx);
        visit(ids[root_idx].0.as_str(), &ids, &mut render_order);
    }

    // Assign monotonic sequences in render order
    for (seq, &idx) in render_order.iter().enumerate() {
        blocks[idx].set_sequence(seq as i64);
    }
}

/// Assert two NormalizedDocuments are equal with good field-by-field error messages.
fn assert_normalized_docs_equal(
    expected: &NormalizedDocument,
    actual: &NormalizedDocument,
    context: &str,
) -> Result<(), TestCaseError> {
    prop_assert_eq!(
        &expected.title,
        &actual.title,
        "[{}] Document titles must match",
        context
    );
    prop_assert_eq!(
        expected.blocks.len(),
        actual.blocks.len(),
        "[{}] Block count must match.\nExpected IDs: {:?}\nActual IDs: {:?}",
        context,
        expected
            .blocks
            .iter()
            .map(|b| b.id.as_str())
            .collect::<Vec<_>>(),
        actual
            .blocks
            .iter()
            .map(|b| b.id.as_str())
            .collect::<Vec<_>>(),
    );

    for exp in &expected.blocks {
        let act = actual
            .blocks
            .iter()
            .find(|b| b.id == exp.id)
            .ok_or_else(|| {
                TestCaseError::Fail(
                    format!(
                        "[{}] Block '{}' missing from actual",
                        context,
                        exp.id.as_str()
                    )
                    .into(),
                )
            })?;

        prop_assert_eq!(
            &exp.content_type,
            &act.content_type,
            "[{}] content_type for '{}'",
            context,
            exp.id
        );

        if exp.content_type == ContentType::Text {
            prop_assert_eq!(
                &exp.title,
                &act.title,
                "[{}] title for '{}'",
                context,
                exp.id
            );
            prop_assert_eq!(exp.level, act.level, "[{}] level for '{}'", context, exp.id);
            prop_assert_eq!(
                &exp.task_state,
                &act.task_state,
                "[{}] task_state for '{}'",
                context,
                exp.id
            );
            prop_assert_eq!(
                exp.priority,
                act.priority,
                "[{}] priority for '{}'",
                context,
                exp.id
            );
            prop_assert_eq!(&exp.tags, &act.tags, "[{}] tags for '{}'", context, exp.id);
            prop_assert_eq!(
                &exp.scheduled,
                &act.scheduled,
                "[{}] scheduled for '{}'",
                context,
                exp.id
            );
            prop_assert_eq!(
                &exp.deadline,
                &act.deadline,
                "[{}] deadline for '{}'",
                context,
                exp.id
            );
            prop_assert_eq!(
                &exp.drawer_properties,
                &act.drawer_properties,
                "[{}] drawer_properties for '{}'",
                context,
                exp.id
            );
        }

        if exp.content_type == ContentType::Source {
            prop_assert_eq!(
                &exp.source_language,
                &act.source_language,
                "[{}] source_language for '{}'",
                context,
                exp.id
            );
            prop_assert_eq!(
                &exp.source_name,
                &act.source_name,
                "[{}] source_name for '{}'",
                context,
                exp.id
            );
            prop_assert_eq!(
                &exp.header_args,
                &act.header_args,
                "[{}] header_args for '{}'",
                context,
                exp.id
            );
            prop_assert_eq!(
                &exp.drawer_properties,
                &act.drawer_properties,
                "[{}] drawer_properties for '{}'",
                context,
                exp.id
            );
        }
    }

    Ok(())
}

// -- TextMutation: text-level mutations applied to org strings ----------------

#[derive(Debug, Clone)]
enum TextMutation {
    ReplaceTitle {
        section_idx: usize,
        new_title: String,
    },
    AddTodoKeyword {
        section_idx: usize,
        keyword: String,
    },
    RemoveTodoKeyword {
        section_idx: usize,
    },
    AddTag {
        section_idx: usize,
        tag: String,
    },
    SetPriority {
        section_idx: usize,
        letter: char,
    },
    RemovePriority {
        section_idx: usize,
    },
}

#[derive(Debug, Clone)]
struct HeadlineSection {
    start_line: usize,
    end_line: usize,
    level: usize,
    id: Option<String>,
}

fn find_headline_sections(org_text: &str) -> Vec<HeadlineSection> {
    let lines: Vec<&str> = org_text.lines().collect();
    let mut sections = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        if line.starts_with('*') {
            let level = line.chars().take_while(|c| *c == '*').count();
            if level > 0 && line.chars().nth(level) == Some(' ') {
                // Find section end (next headline at same or higher level, or EOF)
                let end = lines
                    .iter()
                    .enumerate()
                    .skip(i + 1)
                    .find(|(_, l)| {
                        if l.starts_with('*') {
                            let l_level = l.chars().take_while(|c| *c == '*').count();
                            l_level > 0 && l.chars().nth(l_level) == Some(' ') && l_level <= level
                        } else {
                            false
                        }
                    })
                    .map(|(j, _)| j)
                    .unwrap_or(lines.len());

                // Extract ID from properties drawer
                let id = extract_id_from_section_lines(&lines[i..end]);

                sections.push(HeadlineSection {
                    start_line: i,
                    end_line: end,
                    level,
                    id,
                });
            }
        }
    }

    sections
}

fn extract_id_from_section_lines(lines: &[&str]) -> Option<String> {
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with(":ID:") {
            return Some(trimmed[4..].trim().to_string());
        }
    }
    None
}

fn apply_text_mutation(org_text: &str, mutation: &TextMutation) -> Option<String> {
    let mut lines: Vec<String> = org_text.lines().map(|l| l.to_string()).collect();
    let sections = find_headline_sections(org_text);

    match mutation {
        TextMutation::ReplaceTitle {
            section_idx,
            new_title,
        } => {
            let section = sections.get(*section_idx)?;
            let line = &lines[section.start_line];
            let new_line = replace_headline_title(line, new_title);
            lines[section.start_line] = new_line;
        }
        TextMutation::AddTodoKeyword {
            section_idx,
            keyword,
        } => {
            let section = sections.get(*section_idx)?;
            let line = &lines[section.start_line];
            // Only add if there's no existing TODO keyword
            let after_stars = line.trim_start_matches('*').trim_start();
            let has_todo = DEFAULT_ACTIVE_KEYWORDS
                .iter()
                .chain(DEFAULT_DONE_KEYWORDS.iter())
                .any(|kw| after_stars.starts_with(kw) && after_stars[kw.len()..].starts_with(' '));
            if has_todo {
                return None;
            }
            let stars = "*".repeat(section.level);
            let rest = line[section.level..].trim_start();
            lines[section.start_line] = format!("{} {} {}", stars, keyword, rest);
        }
        TextMutation::RemoveTodoKeyword { section_idx } => {
            let section = sections.get(*section_idx)?;
            let line = &lines[section.start_line];
            let after_stars = line[section.level..].trim_start();
            let removed = DEFAULT_ACTIVE_KEYWORDS
                .iter()
                .chain(DEFAULT_DONE_KEYWORDS.iter())
                .find(|kw| {
                    after_stars.starts_with(*kw) && after_stars[kw.len()..].starts_with(' ')
                });
            match removed {
                Some(kw) => {
                    let stars = "*".repeat(section.level);
                    let rest = after_stars[kw.len()..].trim_start();
                    lines[section.start_line] = format!("{} {}", stars, rest);
                }
                None => return None,
            }
        }
        TextMutation::AddTag { section_idx, tag } => {
            let section = sections.get(*section_idx)?;
            let line = &lines[section.start_line];
            let new_line = add_tag_to_headline(line, tag);
            lines[section.start_line] = new_line;
        }
        TextMutation::SetPriority {
            section_idx,
            letter,
        } => {
            let section = sections.get(*section_idx)?;
            let line = &lines[section.start_line];
            let new_line = set_priority_on_headline(line, section.level, *letter);
            lines[section.start_line] = new_line;
        }
        TextMutation::RemovePriority { section_idx } => {
            let section = sections.get(*section_idx)?;
            let line = &lines[section.start_line];
            let new_line = remove_priority_from_headline(line);
            lines[section.start_line] = new_line;
        }
    }

    // Ensure final newline
    let mut result = lines.join("\n");
    if org_text.ends_with('\n') && !result.ends_with('\n') {
        result.push('\n');
    }
    Some(result)
}

fn replace_headline_title(line: &str, new_title: &str) -> String {
    let level = line.chars().take_while(|c| *c == '*').count();
    let after_stars = line[level..].trim_start();

    // Preserve TODO keyword if present
    let mut prefix_parts = Vec::new();
    let mut rest = after_stars;

    // Check for TODO keyword
    let all_keywords: Vec<&&str> = DEFAULT_ACTIVE_KEYWORDS
        .iter()
        .chain(DEFAULT_DONE_KEYWORDS.iter())
        .collect();
    for kw in &all_keywords {
        if rest.starts_with(**kw) && rest[kw.len()..].starts_with(' ') {
            prefix_parts.push(kw.to_string());
            rest = rest[kw.len()..].trim_start();
            break;
        }
    }

    // Check for priority
    if rest.starts_with("[#") && rest.len() >= 4 && rest.as_bytes()[3] == b']' {
        prefix_parts.push(rest[..4].to_string());
        rest = rest[4..].trim_start();
    }

    // Check for tags at end — preserve them
    let tags_suffix = if let Some(tag_start) = find_tags_suffix(rest) {
        tag_start
    } else {
        ""
    };

    let stars = "*".repeat(level);
    let prefix = prefix_parts.join(" ");
    if prefix.is_empty() {
        if tags_suffix.is_empty() {
            format!("{} {}", stars, new_title)
        } else {
            format!("{} {} {}", stars, new_title, tags_suffix)
        }
    } else if tags_suffix.is_empty() {
        format!("{} {} {}", stars, prefix, new_title)
    } else {
        format!("{} {} {} {}", stars, prefix, new_title, tags_suffix)
    }
}

fn find_tags_suffix(text: &str) -> Option<&str> {
    // Tags look like :tag1:tag2: at the end of the line
    let trimmed = text.trim_end();
    if trimmed.ends_with(':') {
        // Find the start of the tags by searching backwards for a space before ":"
        if let Some(pos) = trimmed.rfind(' ') {
            let candidate = &trimmed[pos + 1..];
            if candidate.starts_with(':')
                && candidate.ends_with(':')
                && candidate.len() > 2
                && candidate.matches(':').count() >= 2
            {
                return Some(candidate);
            }
        }
        // Could also be at start (no space before)
        if trimmed.starts_with(':') && trimmed.matches(':').count() >= 2 {
            return Some(trimmed);
        }
    }
    None
}

fn add_tag_to_headline(line: &str, tag: &str) -> String {
    let trimmed = line.trim_end();
    if trimmed.ends_with(':') {
        // Already has tags, insert before last colon
        if let Some(pos) = trimmed.rfind(' ') {
            let candidate = &trimmed[pos + 1..];
            if candidate.starts_with(':') && candidate.ends_with(':') {
                let before = &trimmed[..pos + 1];
                return format!("{}{}{}:", before, candidate, tag);
            }
        }
    }
    // No existing tags
    format!("{} :{}:", trimmed, tag)
}

fn set_priority_on_headline(line: &str, level: usize, letter: char) -> String {
    let after_stars = line[level..].trim_start();
    let stars = "*".repeat(level);

    // Parse TODO keyword
    let mut rest = after_stars;
    let mut todo = None;
    let all_keywords: Vec<&&str> = DEFAULT_ACTIVE_KEYWORDS
        .iter()
        .chain(DEFAULT_DONE_KEYWORDS.iter())
        .collect();
    for kw in &all_keywords {
        if rest.starts_with(**kw) && rest[kw.len()..].starts_with(' ') {
            todo = Some(kw.to_string());
            rest = rest[kw.len()..].trim_start();
            break;
        }
    }

    // Skip existing priority if present
    if rest.starts_with("[#") && rest.len() >= 4 && rest.as_bytes()[3] == b']' {
        rest = rest[4..].trim_start();
    }

    // Reconstruct
    let mut result = stars;
    result.push(' ');
    if let Some(kw) = todo {
        result.push_str(&kw);
        result.push(' ');
    }
    result.push_str(&format!("[#{}] {}", letter, rest));
    result
}

fn remove_priority_from_headline(line: &str) -> String {
    let level = line.chars().take_while(|c| *c == '*').count();
    let after_stars = line[level..].trim_start();
    let stars = "*".repeat(level);

    let mut rest = after_stars;
    let mut todo = None;
    let all_keywords: Vec<&&str> = DEFAULT_ACTIVE_KEYWORDS
        .iter()
        .chain(DEFAULT_DONE_KEYWORDS.iter())
        .collect();
    for kw in &all_keywords {
        if rest.starts_with(**kw) && rest[kw.len()..].starts_with(' ') {
            todo = Some(kw.to_string());
            rest = rest[kw.len()..].trim_start();
            break;
        }
    }

    // Skip priority if present
    if rest.starts_with("[#") && rest.len() >= 4 && rest.as_bytes()[3] == b']' {
        rest = rest[4..].trim_start();
    }

    let mut result = stars;
    result.push(' ');
    if let Some(kw) = todo {
        result.push_str(&kw);
        result.push(' ');
    }
    result.push_str(rest);
    result
}

/// Apply the equivalent semantic mutation to blocks that corresponds to a TextMutation.
fn apply_equivalent_block_mutation(
    blocks: &mut Vec<Block>,
    mutation: &TextMutation,
    sections: &[HeadlineSection],
) {
    match mutation {
        TextMutation::ReplaceTitle {
            section_idx,
            new_title,
        } => {
            let section = &sections[*section_idx];
            if let Some(id) = &section.id {
                if let Some(block) = blocks.iter_mut().find(|b| b.id.id() == id) {
                    let body = block.body();
                    block.set_title_and_body(new_title.clone(), body);
                }
            }
        }
        TextMutation::AddTodoKeyword {
            section_idx,
            keyword,
        } => {
            let section = &sections[*section_idx];
            if let Some(id) = &section.id {
                if let Some(block) = blocks.iter_mut().find(|b| b.id.id() == id) {
                    block.set_task_state(Some(TaskState::from_keyword(keyword)));
                }
            }
        }
        TextMutation::RemoveTodoKeyword { section_idx } => {
            let section = &sections[*section_idx];
            if let Some(id) = &section.id {
                if let Some(block) = blocks.iter_mut().find(|b| b.id.id() == id) {
                    block.set_task_state(None);
                }
            }
        }
        TextMutation::AddTag { section_idx, tag } => {
            let section = &sections[*section_idx];
            if let Some(id) = &section.id {
                if let Some(block) = blocks.iter_mut().find(|b| b.id.id() == id) {
                    let mut current = block.tags().as_slice().to_vec();
                    current.push(tag.clone());
                    block.set_tags(Tags::from(current));
                }
            }
        }
        TextMutation::SetPriority {
            section_idx,
            letter,
        } => {
            let section = &sections[*section_idx];
            if let Some(id) = &section.id {
                if let Some(block) = blocks.iter_mut().find(|b| b.id.id() == id) {
                    let priority =
                        Priority::from_letter(&letter.to_string()).expect("valid priority letter");
                    block.set_priority(Some(priority));
                }
            }
        }
        TextMutation::RemovePriority { section_idx } => {
            let section = &sections[*section_idx];
            if let Some(id) = &section.id {
                if let Some(block) = blocks.iter_mut().find(|b| b.id.id() == id) {
                    block.set_priority(None);
                }
            }
        }
    }
}

fn text_mutation_strategy(max_sections: usize) -> impl Strategy<Value = TextMutation> {
    // Generate a section index and mutation type
    let idx = 0..max_sections;
    prop_oneof![
        (idx.clone(), valid_title()).prop_map(|(i, t)| TextMutation::ReplaceTitle {
            section_idx: i,
            new_title: t
        }),
        (
            idx.clone(),
            prop_oneof![
                Just("TODO".to_string()),
                Just("DOING".to_string()),
                Just("DONE".to_string()),
            ]
        )
            .prop_map(|(i, kw)| TextMutation::AddTodoKeyword {
                section_idx: i,
                keyword: kw
            }),
        idx.clone()
            .prop_map(|i| TextMutation::RemoveTodoKeyword { section_idx: i }),
        (idx.clone(), valid_tag()).prop_map(|(i, t)| TextMutation::AddTag {
            section_idx: i,
            tag: t
        }),
        (idx.clone(), prop_oneof![Just('A'), Just('B'), Just('C')]).prop_map(|(i, l)| {
            TextMutation::SetPriority {
                section_idx: i,
                letter: l,
            }
        }),
        idx.prop_map(|i| TextMutation::RemovePriority { section_idx: i }),
    ]
}

// -- Mutation PBT: in-memory mutations ---------------------------------------

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 100,
        ..ProptestConfig::default()
    })]

    #[test]
    fn test_in_memory_mutation(
        mut complete_doc in complete_document_strategy(),
        mutation in block_mutation_strategy(),
        target_idx in any::<prop::sample::Index>(),
    ) {
        complete_doc.ensure_todo_keywords_configured();

        // Step 1: Generate → render → parse = baseline (already round-tripped)
        let blocks = complete_doc.all_blocks();
        let org_text = build_org_text(&complete_doc.document, &blocks);
        let baseline = match parse_org(&org_text) {
            Ok(r) => r,
            Err(_) => return Ok(()),
        };
        let mut baseline_blocks = baseline.blocks;

        // Only target text blocks for most mutations
        let text_block_indices: Vec<usize> = baseline_blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| b.content_type == ContentType::Text)
            .map(|(i, _)| i)
            .collect();

        prop_assume!(!text_block_indices.is_empty());

        // Step 2: Apply mutation
        match &mutation {
            BlockMutation::SwapSiblingOrder => {
                match find_swappable_text_siblings(&baseline_blocks) {
                    Some((a, b)) => {
                        let seq_a = baseline_blocks[a].sequence();
                        let seq_b = baseline_blocks[b].sequence();
                        baseline_blocks[a].set_sequence(seq_b);
                        baseline_blocks[b].set_sequence(seq_a);
                    }
                    None => return Ok(()), // No swappable siblings
                }
            }
            other => {
                let idx = target_idx.index(text_block_indices.len());
                let block_idx = text_block_indices[idx];

                // Constrain task state mutations to default keywords only
                if let BlockMutation::SetTaskState(Some(ts)) = other {
                    let is_default = DEFAULT_ACTIVE_KEYWORDS.contains(&ts.keyword.as_str())
                        || DEFAULT_DONE_KEYWORDS.contains(&ts.keyword.as_str());
                    prop_assume!(is_default);
                }

                apply_mutation(&mut baseline_blocks[block_idx], other);
            }
        }

        // Step 3: Build expected state
        let mut expected_blocks = baseline_blocks.clone();
        reassign_parser_sequences(&mut expected_blocks);
        let expected = NormalizedDocument::from_doc_and_blocks(&baseline.document, &expected_blocks);

        // Step 4: Re-render mutated blocks → re-parse = actual
        let mutated_org_text = build_org_text(&baseline.document, &baseline_blocks);
        let actual_result = match parse_org(&mutated_org_text) {
            Ok(r) => r,
            Err(e) => {
                prop_assert!(false, "Re-parse failed: {}\n\nOrg text:\n{}", e, mutated_org_text);
                return Ok(());
            }
        };
        let actual = NormalizedDocument::from_doc_and_blocks(&actual_result.document, &actual_result.blocks);

        // Step 5: Assert full equality
        assert_normalized_docs_equal(&expected, &actual, "in_memory_mutation")?;

        // Step 6: Render stability — render the parsed result again, text must be identical.
        // This catches sort-order bugs where a single render→parse is self-consistent
        // but the ordering doesn't converge to a fixed point.
        let re_rendered = build_org_text(&actual_result.document, &actual_result.blocks);
        prop_assert_eq!(
            &mutated_org_text,
            &re_rendered,
            "Render must be idempotent after mutation.\n\n=== FIRST RENDER ===\n{}\n=== SECOND RENDER ===\n{}",
            mutated_org_text,
            re_rendered,
        );
    }
}

// -- Mutation PBT: org text mutations ----------------------------------------

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 100,
        ..ProptestConfig::default()
    })]

    #[test]
    fn test_org_text_mutation(
        mut complete_doc in complete_document_strategy(),
        mutation_kind in 0..6u8,
        title in valid_title(),
        tag in valid_tag(),
        priority_letter in prop_oneof![Just('A'), Just('B'), Just('C')],
        todo_keyword in prop_oneof![Just("TODO".to_string()), Just("DOING".to_string()), Just("DONE".to_string())],
        target_idx in any::<prop::sample::Index>(),
    ) {
        complete_doc.ensure_todo_keywords_configured();

        // Step 1: Generate → render → parse = baseline
        let blocks = complete_doc.all_blocks();
        let org_text = build_org_text(&complete_doc.document, &blocks);
        let baseline = match parse_org(&org_text) {
            Ok(r) => r,
            Err(_) => return Ok(()),
        };

        // Re-render from parsed baseline to get stable org text
        let stable_org = build_org_text(&baseline.document, &baseline.blocks);

        // Find headline sections in the stable org text
        let sections = find_headline_sections(&stable_org);
        prop_assume!(!sections.is_empty());

        let section_count = sections.len();
        let idx = target_idx.index(section_count);

        // Build the text mutation
        let mutation = match mutation_kind {
            0 => TextMutation::ReplaceTitle { section_idx: idx, new_title: title },
            1 => TextMutation::AddTodoKeyword { section_idx: idx, keyword: todo_keyword },
            2 => TextMutation::RemoveTodoKeyword { section_idx: idx },
            3 => TextMutation::AddTag { section_idx: idx, tag },
            4 => TextMutation::SetPriority { section_idx: idx, letter: priority_letter },
            _ => TextMutation::RemovePriority { section_idx: idx },
        };

        // Step 2: Apply text mutation to org string
        let mutated_org = match apply_text_mutation(&stable_org, &mutation) {
            Some(text) => text,
            None => return Ok(()), // Mutation not applicable
        };

        // Step 3: Parse mutated org text = actual
        let actual_result = match parse_org(&mutated_org) {
            Ok(r) => r,
            Err(e) => {
                prop_assert!(false, "Parse of mutated text failed: {}\n\nMutated org:\n{}", e, mutated_org);
                return Ok(());
            }
        };
        let actual = NormalizedDocument::from_doc_and_blocks(&actual_result.document, &actual_result.blocks);

        // Step 4: Apply equivalent block mutation = expected
        let mut expected_blocks = baseline.blocks.clone();
        apply_equivalent_block_mutation(&mut expected_blocks, &mutation, &sections);
        reassign_parser_sequences(&mut expected_blocks);
        let expected = NormalizedDocument::from_doc_and_blocks(&baseline.document, &expected_blocks);

        // Step 5: Assert full equality
        assert_normalized_docs_equal(&expected, &actual, "org_text_mutation")?;
    }
}
