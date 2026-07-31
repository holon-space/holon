//! @c4 component
//! @c4 layer Testing
//! Pattern: Test Harness
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-core "core datasource traits" "Rust"
//! @c4 uses holon-org-format "org parse/render" "Rust"
//!
//! Format-and-storage-agnostic generators and normalized comparison shapes
//! for block round-trip property tests.
//!
//! This crate carries the shared *generator* surface used by every block
//! round-trip PBT: org file round-trip, Turso storage round-trip, and any
//! future format adapters (markdown, …). Each PBT picks its own write/read
//! pair (`OrgFormatAdapter`, `BlockReader` + `OperationProvider`, …) and
//! re-uses these generators + the `NormalizedDocument` comparison shape so
//! the property — *what goes in comes out* — is identical across backends.
//!
//! No org/markdown/turso specifics live here. The shape stays pure data:
//! `Block` instances and their relationships. Format-specific concerns
//! (TODO-keyword propagation, corpus seeding from on-disk files, render
//! and parse helpers) stay in the per-format test crates.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;

use holon_api::ContentType;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::Priority;
use holon_api::SourceLanguage;
use holon_api::Tags;
use holon_api::TaskState;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
// OrgBlockExt provides domain-level accessors (level/tags/priority/task_state/
// scheduled/deadline/sequence/drawer_properties) plus their setters. Used
// internally; not re-exported.
use holon_org_format::models::OrgBlockExt;
use proptest::prelude::*;
use uuid::Uuid;

/// Deterministic timestamp for generated fixture blocks (2024-01-01T00:00:00Z)
/// so strategies stay reproducible under proptest replay and shrinking.
const FIXED_FIXTURE_TIMESTAMP_MS: i64 = 1_704_067_200_000;

// ============================================================================
// Normalized representation for comparison
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedBlock {
    pub id: EntityUri,
    pub parent_id: EntityUri,
    pub content_type: ContentType,
    // Headline fields
    pub level: i64,
    pub title: String,
    pub task_state: Option<TaskState>,
    pub priority: Option<Priority>,
    pub tags: BTreeSet<String>,
    // Source block fields
    pub source_content: String,
    pub source_language: Option<String>,
    pub source_name: Option<String>,
    pub header_args: BTreeMap<String, String>,
    // Planning timestamps
    pub scheduled: Option<String>,
    pub deadline: Option<String>,
    // Custom drawer properties (non-internal keys like column-order, collapse-to, etc.)
    pub drawer_properties: BTreeMap<String, String>,
    // Ordering
    pub sequence: i64,
}

impl NormalizedBlock {
    pub fn from_block(block: &Block) -> Self {
        // Title = first line of content. Mirrors `OrgBlockExt::org_title`
        // (defined in holon-org-format) inline so this crate has no
        // format-crate dependency.
        let title = block
            .content
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string();

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
            source_content: block.content.clone(),
            source_language: block.source_language.as_ref().map(|l| l.to_string()),
            source_name: block.source_name.clone(),
            header_args,
            drawer_properties,
            sequence: block.sequence(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedDocument {
    pub title: Option<String>,
    pub blocks: Vec<NormalizedBlock>,
}

impl NormalizedDocument {
    /// Build a normalized document from blocks + an explicit title.
    ///
    /// The title is passed in directly (rather than extracted from a `Block`)
    /// because document-title accessors live in format-specific extension
    /// traits (e.g. `OrgDocumentExt::file_title` in `holon-orgmode`). Callers
    /// in format-specific test crates pass `doc.file_title()` etc.; callers
    /// in storage-only tests can pass `None`.
    pub fn from_blocks(title: Option<String>, blocks: &[Block]) -> Self {
        let mut normalized_blocks: Vec<NormalizedBlock> =
            blocks.iter().map(NormalizedBlock::from_block).collect();
        normalized_blocks.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));

        NormalizedDocument {
            title: title.map(|t| t.trim().to_string()),
            blocks: normalized_blocks,
        }
    }

    /// Build a normalized document from a [`BlockSnapshot`] capture — the shape
    /// every [`BlockQuerySource`] produces. This is the bridge for round-trip
    /// PBTs that read a backend through `BlockQuerySource::snapshot()` (Turso's
    /// CDC mirrors, a Loro reader, an in-memory reference) and compare it,
    /// field-for-field and id-keyed, against the generated blocks.
    ///
    /// [`BlockSnapshot`]: holon_core::storage::BlockSnapshot
    /// [`BlockQuerySource`]: holon_core::storage::BlockQuerySource
    pub fn from_block_snapshot(
        title: Option<String>,
        snapshot: &holon_core::storage::BlockSnapshot,
    ) -> Self {
        let blocks: Vec<Block> = snapshot.iter_blocks().cloned().collect();
        Self::from_blocks(title, &blocks)
    }
}

/// Build a reference [`BlockSnapshot`] from generated blocks (in their
/// document/`sequence` order) plus focus-roots, for `reference == actual`
/// comparison against a backend's `BlockQuerySource::snapshot()`.
///
/// [`BlockSnapshot`]: holon_core::storage::BlockSnapshot
pub fn reference_block_snapshot(
    blocks: &[Block],
    focus_roots: Vec<holon_core::storage::FocusRoot>,
) -> holon_core::storage::BlockSnapshot {
    holon_core::storage::BlockSnapshot::from_ordered(blocks.iter().cloned(), focus_roots)
}

/// Assert that every parent's children appear in the same order in `actual`
/// (a backend capture) as in the generated `blocks` (document order).
///
/// `NormalizedDocument` equality is id-keyed and order-independent — it locks
/// *fields*. This locks *sibling order*: for each parent, the sequence of child
/// ids `actual.children_ordered(parent)` must equal the order the generator
/// emitted them.
pub fn assert_sibling_order_matches(
    blocks: &[Block],
    actual: &holon_core::storage::BlockSnapshot,
    context: &str,
) -> Result<(), TestCaseError> {
    use holon_core::storage::BlockQuery;

    // Expected per-parent child order from the generated (document-order) blocks.
    let mut expected_children: BTreeMap<EntityUri, Vec<EntityUri>> = BTreeMap::new();
    for b in blocks {
        expected_children
            .entry(b.parent_id.clone())
            .or_default()
            .push(b.id.clone());
    }

    for (parent, expected_ids) in &expected_children {
        let actual_ids: Vec<EntityUri> = actual
            .children_ordered(parent)
            .into_iter()
            .map(|b| b.id)
            .collect();
        prop_assert_eq!(
            expected_ids,
            &actual_ids,
            "[{}] sibling order under parent '{}'",
            context,
            parent.as_str()
        );
    }
    Ok(())
}

// ============================================================================
// Strategy: Valid identifiers and text
// ============================================================================

pub fn valid_identifier() -> impl Strategy<Value = String> {
    "[a-zA-Z][a-zA-Z0-9_]{0,19}[a-zA-Z0-9]?"
}

pub fn valid_tag() -> impl Strategy<Value = String> {
    // Tags must match the parser's permitted character set
    // (see holon-space/orgize `ParseConfig::is_tag_char` default):
    // alphanumeric + `_@#%-`. The upstream org-element.el spec only allows
    // `_@#%`, so against vanilla orgize hyphenated tags are silently dropped.
    // We bias towards the spec shapes but also exercise hyphens — real
    // corpora (Logseq / Org-Roam / this project's own AC-N notes) routinely
    // emit tags like `:G1:edge-abstraction:`.
    prop_oneof![
        // Spec-conformant alphanumeric/underscore tags.
        2 => "[a-zA-Z][a-zA-Z0-9_]{0,14}",
        // Hyphenated tags (the holon-space/orgize fork enables these).
        1 => "[a-zA-Z][a-zA-Z0-9_-]{0,13}[a-zA-Z0-9]",
    ]
}

/// Text whose bytes COLLIDE with org inline markup — `__init__`, `*bold*`,
/// `=verbatim=`, and the same shapes embedded in a sentence.
///
/// The store holds these as LITERALS (no marks): they arrive from seeds, MCP
/// `block.create` and typing, never from the org parser. A round trip that
/// emits them unquoted parses them back as emphasis and drops the delimiters,
/// which is one-shot data loss. Purely alphanumeric character classes cannot
/// reach this shape, so it gets its own weighted arm.
pub fn markup_shaped_literal() -> impl Strategy<Value = String> {
    (
        prop_oneof![
            Just("_"),
            Just("__"),
            Just("*"),
            Just("**"),
            Just("/"),
            Just("~"),
            Just("="),
            Just("+"),
        ],
        "[a-z][a-z0-9]{0,10}",
        prop::option::of("[a-z]{1,6}"),
        prop::option::of("[a-z]{1,6}"),
    )
        .prop_map(|(marker, ident, before, after)| {
            let core = format!("{marker}{ident}{marker}");
            match (before, after) {
                (None, None) => core,
                (Some(b), None) => format!("{b} {core}"),
                (None, Some(a)) => format!("{core} {a}"),
                (Some(b), Some(a)) => format!("{b} {core} {a}"),
            }
        })
}

pub fn valid_title() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => "[a-zA-Z0-9][a-zA-Z0-9 ]{0,48}[a-zA-Z0-9]",
        1 => markup_shaped_literal(),
    ]
}

pub fn valid_body() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => "[a-zA-Z0-9 .,!?\n]{10,200}",
        1 => markup_shaped_literal(),
    ]
}

// ============================================================================
// Strategy: (content, marks) store states
// ============================================================================

/// A generated STORE STATE — content plus the mark set a block carries with
/// it. The whole point is that `marks` is minted INDEPENDENTLY of any parse.
///
/// Marks reach a block from many producers that never consult the org parser:
/// Peritext reads off a `LoroText`, block-split re-anchoring, template
/// instantiation, the operation engine, the markdown adapters — and the org
/// renderer's own literal-quoting, which mints a `Verbatim` the store then
/// hands back. A generator that only ever produces marks by parsing org text
/// can therefore never reach most of the states production reaches, which is
/// exactly how a co-extensive-mark corruption survived three review rounds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkedContent {
    pub content: String,
    pub marks: Vec<MarkSpan>,
}

/// Where a generated mark sits relative to a markup-shaped literal in the
/// content. These are the geometries that break renderers, so they are
/// generated by CONSTRUCTION rather than left to chance: a uniform random span
/// almost never lands exactly on a literal's boundaries.
#[derive(Debug, Clone, Copy)]
enum MarkGeometry {
    /// Exactly the literal — the shape a user "bolds this identifier" mints,
    /// and the shape the renderer's own quoting mints on re-ingest.
    CoExtensive,
    /// Ends where the literal starts / starts where it ends.
    BoundaryBefore,
    BoundaryAfter,
    /// Strictly contains the literal, with slack on both sides.
    ContainingWithSlack,
    /// Strictly inside the literal.
    Inside,
    /// Covers the literal's first half only — org cannot express this.
    Crossing,
    /// The whole content.
    Whole,
}

fn mark_kind_strategy() -> impl Strategy<Value = InlineMark> {
    prop_oneof![
        Just(InlineMark::Bold),
        Just(InlineMark::Italic),
        Just(InlineMark::Underline),
        Just(InlineMark::Verbatim),
        Just(InlineMark::Code),
        Just(InlineMark::Strike),
    ]
}

/// Content built from alternating plain and markup-shaped segments, paired
/// with an arbitrary mark set whose spans are weighted onto the adversarial
/// geometries above.
pub fn marked_content_strategy() -> impl Strategy<Value = MarkedContent> {
    let segment = prop_oneof![
        2 => "[a-z]{1,6}".prop_map(|w| (w, false)),
        3 => markup_shaped_literal().prop_map(|w| (w, true)),
    ];
    let geometry = prop_oneof![
        // Co-extensive and crossing are the two that break naive renderers;
        // weight them up so a few hundred cases reliably hit both.
        3 => Just(MarkGeometry::CoExtensive),
        3 => Just(MarkGeometry::Crossing),
        1 => Just(MarkGeometry::BoundaryBefore),
        1 => Just(MarkGeometry::BoundaryAfter),
        1 => Just(MarkGeometry::ContainingWithSlack),
        1 => Just(MarkGeometry::Inside),
        1 => Just(MarkGeometry::Whole),
    ];
    (
        prop::collection::vec(segment, 1..4),
        prop::collection::vec(
            (geometry, mark_kind_strategy(), any::<prop::sample::Index>()),
            0..4,
        ),
    )
        .prop_map(|(segments, mark_specs)| {
            let content = segments
                .iter()
                .map(|(text, _)| text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let total = content.chars().count();
            // Char range of every segment, and of the literal ones alone.
            let mut ranges = Vec::new();
            let mut cursor = 0usize;
            for (text, is_literal) in &segments {
                let len = text.chars().count();
                ranges.push((cursor, cursor + len, *is_literal));
                cursor += len + 1;
            }
            let literals: Vec<(usize, usize)> = ranges
                .iter()
                .filter(|(_, _, is_literal)| *is_literal)
                .map(|(s, e, _)| (*s, *e))
                .collect();
            let anchors = if literals.is_empty() {
                ranges.iter().map(|(s, e, _)| (*s, *e)).collect()
            } else {
                literals
            };

            let marks = mark_specs
                .into_iter()
                .filter_map(|(geometry, kind, idx)| {
                    let (start, end) = anchors[idx.index(anchors.len())];
                    let (s, e) = match geometry {
                        MarkGeometry::CoExtensive => (start, end),
                        MarkGeometry::BoundaryBefore => (start.saturating_sub(3), start),
                        MarkGeometry::BoundaryAfter => (end, (end + 3).min(total)),
                        MarkGeometry::ContainingWithSlack => {
                            (start.saturating_sub(2), (end + 2).min(total))
                        }
                        MarkGeometry::Inside => (start + 1, end.saturating_sub(1)),
                        MarkGeometry::Crossing => (start, start + (end - start) / 2),
                        MarkGeometry::Whole => (0, total),
                    };
                    (s < e && e <= total).then(|| MarkSpan {
                        start: s,
                        end: e,
                        mark: kind,
                    })
                })
                .collect();
            MarkedContent { content, marks }
        })
}

pub fn valid_source_code() -> impl Strategy<Value = String> {
    // Must start with non-whitespace (orgize strips leading blank lines from source
    // content) Must not end with '\n': the wire format requires exactly one
    // '\n' before `#+END_SRC`, and the parser strips it on extraction. Enforce
    // the "content never ends with newline" invariant here so round-trip holds.
    prop_oneof![
        // Normal source code (no special chars)
        3 => "[a-zA-Z0-9_=(){}\\[\\];,.][a-zA-Z0-9_ =(){}\\[\\];,.\n]{9,99}",
        // Source code containing lines that need comma-escaping:
        // lines starting with * or #+ must be escaped as ,* / ,#+
        1 => prop_oneof![
            Just("code line\n#+END_SRC\nmore code".to_string()),
            Just("* headline-like line\ncode".to_string()),
            Just("x = 1\n#+TITLE: fake\n#+BEGIN_SRC nested\ny = 2".to_string()),
            Just("normal\n* star line\n#+RESULTS: fake".to_string()),
        ],
    ]
    .prop_map(|s| s.trim_end_matches('\n').to_string())
}

pub fn valid_property_value() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_-]{1,30}"
}

pub fn valid_timestamp() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("<2024-01-15 Mon>".to_string()),
        Just("<2024-06-20 Thu 14:00>".to_string()),
        Just("<2024-12-31 Tue 09:30>".to_string()),
    ]
}

// ============================================================================
// Strategy: Properties drawer (with explicit :ID:)
// ============================================================================

#[derive(Debug, Clone)]
pub struct PropertiesDrawer {
    /// Explicit :ID: in the drawer. None means "no :ID: in the org properties
    /// drawer" — the renderer will inject one from block.id.
    pub explicit_id: Option<String>,
    pub other_props: HashMap<String, String>,
}

pub fn properties_drawer_strategy() -> impl Strategy<Value = PropertiesDrawer> {
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
pub struct SourceBlockSpec {
    pub id: EntityUri,
    pub language: String,
    pub source: String,
    pub name: Option<String>,
    pub header_args: HashMap<String, String>,
    pub custom_properties: HashMap<String, String>,
}

pub fn source_block_spec_strategy() -> impl Strategy<Value = SourceBlockSpec> {
    (
        prop_oneof![
            Just("holon_prql".to_string()),
            Just("python".to_string()),
            Just("rust".to_string()),
            Just("holon_sql".to_string()),
            Just("render".to_string()),
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
        any::<u128>(),
    )
        .prop_map(
            |(language, source, name, header_args, custom_properties, id_bits)| SourceBlockSpec {
                id: EntityUri::block(&Uuid::from_u128(id_bits).to_string()),
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
pub struct HeadlineSpec {
    /// Internal block ID. Used as Block.id.
    pub block_id: EntityUri,
    pub properties_drawer: PropertiesDrawer,
    pub level: i64,
    pub task_state: Option<TaskState>,
    pub priority: Option<Priority>,
    pub title: String,
    pub tags: Option<Vec<String>>,
    pub body: Option<String>,
    pub scheduled: Option<String>,
    pub deadline: Option<String>,
    pub source_blocks: Vec<SourceBlockSpec>,
    pub child_headlines: Vec<HeadlineSpec>,
}

impl HeadlineSpec {
    pub fn id(&self) -> &EntityUri {
        &self.block_id
    }

    pub fn to_block(&self, parent_id: &EntityUri, sequence: &mut i64) -> Vec<Block> {
        let mut blocks = Vec::new();

        let content = match &self.body {
            Some(b) => format!("{}\n{}", self.title, b),
            None => self.title.clone(),
        };

        let mut block = Block::new_text(self.id().clone(), parent_id.clone(), &content);
        block.set_level(self.level);
        block.set_sequence(*sequence);
        *sequence += 1;

        block.set_task_state(self.task_state.clone());
        block.set_priority(self.priority);

        if let Some(ref tags) = self.tags {
            if !tags.is_empty() {
                block.set_tags(Tags::from(tags.clone()));
            }
        }

        block.set_scheduled(self.scheduled.as_deref().map(|s| {
            holon_api::types::Timestamp::parse(s).unwrap_or_else(|e| {
                panic!("generated scheduled timestamp {s:?} failed to parse: {e}")
            })
        }));
        block.set_deadline(self.deadline.as_deref().map(|s| {
            holon_api::types::Timestamp::parse(s).unwrap_or_else(|e| {
                panic!("generated deadline timestamp {s:?} failed to parse: {e}")
            })
        }));

        // Set org properties as flat keys (only include :ID: if explicitly set in
        // drawer)
        if let Some(ref explicit_id) = self.properties_drawer.explicit_id {
            block.set_property("ID", Value::String(explicit_id.clone()));
        }
        for (k, v) in &self.properties_drawer.other_props {
            block.set_property(k, Value::String(v.clone()));
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
                content: sb_spec.source.clone(),
                content_type: ContentType::Source,
                source_language: Some(sb_spec.language.parse::<SourceLanguage>().unwrap()),
                source_name: sb_spec.name.clone(),
                created_at: FIXED_FIXTURE_TIMESTAMP_MS,
                updated_at: FIXED_FIXTURE_TIMESTAMP_MS,
                ..Block::default()
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
            blocks.extend(child.to_block(self.id(), sequence));
        }

        blocks
    }
}

pub fn headline_spec_strategy(
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
            Just(Priority::Low),
            Just(Priority::Medium),
            Just(Priority::High),
        ]),
        valid_title(),
        prop::option::of(prop::collection::vec(valid_tag(), 1..=3)),
        prop::option::of(valid_body()),
        prop::option::of(valid_timestamp()),
        prop::option::of(valid_timestamp()),
        prop::collection::vec(source_block_spec_strategy(), 0..=3),
        any::<u128>(),
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
                fallback_id_bits,
            )| {
                // Use explicit_id from drawer if present, otherwise generate a fresh UUID
                let raw_id = props
                    .explicit_id
                    .clone()
                    .unwrap_or_else(|| Uuid::from_u128(fallback_id_bits).to_string());
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
// Strategy: Block tree (random root headlines under a given parent)
// ============================================================================

/// Strategy for a vec of root `HeadlineSpec`s — the format-neutral block-tree
/// shape. Wrap with format-specific document construction in each test file.
///
/// `level` defaults to 1 (top-level headlines), `max_children` and `max_depth`
/// match the original test's defaults of 2 and 2.
pub fn root_headlines_strategy() -> impl Strategy<Value = Vec<HeadlineSpec>> {
    prop::collection::vec(headline_spec_strategy(1, 2, 2), 1..=4)
}

/// Build a flat `Vec<Block>` from a parent id + a list of root headlines.
/// Mirrors what the original `CompleteDocument::all_blocks` did.
pub fn build_blocks(parent_id: &EntityUri, headlines: &[HeadlineSpec]) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut sequence = 0i64;

    for headline in headlines {
        blocks.extend(headline.to_block(parent_id, &mut sequence));
    }

    blocks
}

// ============================================================================
// Shared assertion: two normalized documents must be equal
// ============================================================================

/// Assert two `NormalizedDocument`s are equal, with field-by-field error
/// messages. Designed to be called inside a `proptest!` body — propagates
/// `TestCaseError` via `?`.
///
/// `context` is a short label included in every error message (e.g.
/// `"org_round_trip"`, `"turso_round_trip"`) so multi-stage PBTs can identify
/// which assertion failed.
///
/// Compares: title, block count, and per-block `(content_type, title, level,
/// task_state, priority, tags, scheduled, deadline, drawer_properties,
/// source_language, source_name, header_args)`. Skips `parent_id` because
/// per-format tests need different root-id handling (e.g. org parser
/// generates a new document block id; Turso preserves the original).
pub fn assert_normalized_docs_equal(
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
        // TODO: Can we compare the entire struct?
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

// Helpers

/// Recursively collect explicit :ID: values from headline specs.
pub fn collect_explicit_ids(headline: &HeadlineSpec) -> Vec<String> {
    let mut ids = Vec::new();
    if let Some(ref id) = headline.properties_drawer.explicit_id {
        ids.push(id.clone());
    }
    for child in &headline.child_headlines {
        ids.extend(collect_explicit_ids(child));
    }
    ids
}
