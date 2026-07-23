//! Proptest strategy generators for PBT transitions.
//!
//! @pbt kind generator
//! @pbt gen content strategies (content/edit/typing/bulk) mix the multi-byte /
//!   empty / whitespace / org-special `extended_content_arm` UNCONDITIONALLY at
//!   ~40-50% — it is NOT behind HOLON_PBT_EXTENDED_GEN. Only three arms are
//!   env-gated: file-tree nesting (parent-selector), GQL query language
//!   (generate_query_language), and the profile-in-no-override file arm. (Prior
//!   header claim that byte-offset classes are ASCII-hidden by default was
//!   stale — those classes ARE in the default gate.)
//! @pbt covers content-vocabulary — shared ADVICE_TAG_POOL keeps advice
//!   invariants non-vacuous (anchor/candidate tag collisions)
//! @pbt gen file-tree is FLAT by default (every heading parents to doc root);
//!   the parent-selector arm (heading i → sel % (i+1)) only fires under
//!   HOLON_PBT_EXTENDED_GEN, so headline-depth / nested-page bug classes are
//!   NOT reached by the default gate (runtime nesting only via Create/mutation)
//! @pbt gen ADVICE_TAG_POOL couples two floors: rule-block presence IS floored
//!   (advice_rule_arm_reachable_in_default_mix ~20/200) but the tag COLLISION
//!   that makes advice_expectation nonempty is NOT — a SetEdgeField weight/pool
//!   change can silently drop advice invariants to the exp-empty vacuous path

use std::collections::HashMap;
use std::collections::HashSet;

use holon_api::ContentType;
use holon_api::EntityUri;
use holon_api::QueryLanguage;
use holon_api::SourceLanguage;
use holon_api::TaskState;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::predicate::Predicate;
use holon_api::vault_shape::VAULT_SHAPE_SCHEMA_VERSION;
use holon_api::vault_shape::VaultShapeProfile;
use holon_orgmode::models::OrgBlockExt;
use holon_pbt_core::content_generators::extended_char;
use holon_pbt_core::content_generators::extended_content_arm;
use holon_pbt_core::types::Mutation;
use proptest::prelude::*;

use super::query::QuerySource;
use super::query::QueryTable;
use super::query::TestQuery;
use super::reference_state::VALID_PROFILE_YAMLS;
use super::reference_state::valid_render_expression_strings;

/// Shared tag vocabulary. Drawn from by BOTH the advice-rule-minting arm
/// (`file_with_advice_rule`) and `SetEdgeField`'s pool tag sub-arm, so that a
/// rule's anchor/candidate tags and the tags landed on blocks collide with
/// non-trivial probability — that collision is what makes advice expectations
/// nonempty. Non-vacuity of the advice invariants depends on this shared pool.
/// All lowercase (`PAGE_TAG` is `"Page"`), so a pool tag can never flip
/// `is_page`.
pub const ADVICE_TAG_POOL: &[&str] = &["task", "lesson", "proj", "urgent", "ctx"];

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

/// Profile-driven shape widening (`HOLON_PBT_SHAPE_PROFILE=<path-to-json>`).
///
/// When set, the generated vault's per-file block count, headline content
/// length and (via block count feeding the parent-selector) tree depth widen
/// toward the shape of a REAL vault — the environment-parity lever
/// (ENV dominates the escape funnel). Unset ⇒ the historical hardcoded bounds,
/// so the blessed default gates stay byte-identical. The profile is read at
/// strategy BUILD time and cached; the bounds only ever RAISE range upper
/// bounds, so proptest still shrinks every range toward its small end and
/// shrink quality is preserved regardless of the active profile.
pub fn active_shape_profile() -> Option<&'static VaultShapeProfile> {
    static PROFILE: std::sync::OnceLock<Option<VaultShapeProfile>> = std::sync::OnceLock::new();
    PROFILE
        .get_or_init(|| {
            let path = std::env::var("HOLON_PBT_SHAPE_PROFILE").ok()?;
            let json = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("HOLON_PBT_SHAPE_PROFILE={path}: {e}"));
            let profile: VaultShapeProfile = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("HOLON_PBT_SHAPE_PROFILE={path} parse error: {e}"));
            assert_eq!(
                profile.schema_version, VAULT_SHAPE_SCHEMA_VERSION,
                "shape profile schema mismatch (regenerate with the current extractor)"
            );
            eprintln!(
                "[HOLON_PBT_SHAPE_PROFILE] ACTIVE ({path}): blocks/file<={} content<={} \
                 depth-reach<={}",
                blocks_per_file_gen_bound_for(&profile),
                content_len_gen_bound_for(&profile),
                profile.depth_bound(),
            );
            Some(profile)
        })
        .as_ref()
}

/// Runtime ceiling on the per-file block count, so one profile-driven case
/// stays tractable inside the 600s smoke gate even when the real vault's p95 is
/// far larger. Disclosed cap, not a silent clamp.
const MAX_BLOCKS_PER_FILE_GEN: u32 = 24;
/// Runtime ceiling on generated headline content length (chars).
const MAX_CONTENT_LEN_GEN: u32 = 128;

fn blocks_per_file_gen_bound_for(p: &VaultShapeProfile) -> u32 {
    p.blocks_per_file_bound().clamp(1, MAX_BLOCKS_PER_FILE_GEN)
}

fn content_len_gen_bound_for(p: &VaultShapeProfile) -> u32 {
    p.content_length_bound().clamp(1, MAX_CONTENT_LEN_GEN)
}

/// Per-file block-count range upper bound: historical `5`, or the (capped)
/// profile p95 when a shape profile is active.
pub fn blocks_per_file_gen_bound() -> usize {
    match active_shape_profile() {
        Some(p) => blocks_per_file_gen_bound_for(p) as usize,
        None => 5,
    }
}

/// Max total headline-content chars: historical `21` (`[A-Z]` + `{0,20}`), or
/// the (capped) profile p95 when a shape profile is active.
pub fn content_len_gen_bound() -> u32 {
    match active_shape_profile() {
        Some(p) => content_len_gen_bound_for(p),
        None => 21,
    }
}

/// The base headline-content strategy, width driven by
/// [`content_len_gen_bound`]. Same character class as before (single-line, no
/// newline), just a wider length ceiling under a profile.
fn content_base_regex() -> BoxedStrategy<String> {
    let tail = content_len_gen_bound().saturating_sub(1);
    proptest::string::string_regex(&format!("[A-Z][a-zA-Z0-9 ]{{0,{tail}}}"))
        .expect("content base regex is valid")
        .boxed()
}

/// Generate single-line block content for headlines.
/// Headlines must be single-line because the org parser treats newlines in
/// headline text as content boundaries — multi-line headlines cause
/// `:PROPERTIES:` drawers to be embedded in the content.
pub fn content_strategy() -> BoxedStrategy<String> {
    let base = content_base_regex();
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
        // Wiki-name link markup typed straight into the editor. This is the
        // ONLY editor-driven path (TypeChars → set_field("content") → the
        // dispatcher's `extract_inline_marks`) that mints a `Link` mark, so it
        // is what exercises the (content, marks) pair through the mark-aware
        // write + undo inverse. Without it the mark column stays empty on every
        // draw and `inv-blocks-match-ref/block_raw` (which DOES compare marks)
        // can never observe a link-mark divergence — the undo-drops-marks bug
        // (Mac dogfood 2026-07-19) was structurally unreachable for this reason.
        2 => "[a-z]{2,5}".prop_map(|w| format!("[[{w}]]")),
    ]
    .boxed()
}

/// Externally-added block content (`BulkExternalAdd`). Extended mode adds
/// the full extended-content arm via the external write path.
pub fn bulk_content_strategy() -> BoxedStrategy<String> {
    let base = "[a-zA-Z][a-zA-Z0-9 ]{0,20}".prop_map(|s| s).boxed();
    prop_oneof![6 => base, 4 => extended_content_arm()].boxed()
}

/// Build the blocks for an index.org heading with a query source + render
/// source.
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

/// Uniquify a sequence of raw block ids in document order. Org files require
/// unique `:ID:`s, so duplicate random ids are an illegal input state — made
/// unrepresentable by suffixing a repeat with its position index until fresh
/// (still matches `[a-z0-9-]+`). Without this, two blocks share an id and every
/// by-id consumer (ref model, the round-trip test's find-by-id) resolves the
/// wrong block — the keyword_set round-trip red of 2026-07-02.
fn uniquify_ids(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    ids.into_iter()
        .enumerate()
        .map(|(i, id)| {
            let mut unique = id;
            while !seen.insert(unique.clone()) {
                unique = format!("{unique}-{i}");
            }
            unique
        })
        .collect()
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
    allow_advice_rule: bool,
) -> BoxedStrategy<(String, Vec<Block>)> {
    use proptest::collection::vec as prop_vec;

    let ks = keyword_set.clone();
    // Generate headlines with optional task states: (headline, id,
    // maybe_task_state_index) ~50% of headlines get a random task keyword when
    // a keyword_set is present.
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
                // construction. Nesting is always active; widening the
                // per-file block count (via a shape profile) widens the
                // reachable tree depth toward real-vault shape.
                prop::num::u8::ANY,
            ),
            1..=blocks_per_file_gen_bound(),
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
            // Uniquified (see `uniquify_ids`): org files require unique `:ID:`s.
            let ids = uniquify_ids(headings.iter().map(|(_, id, _, _, _)| id.clone()));
            // First pass: resolve each heading's parent. Flat (doc root) by
            // default; under extended gen, heading `i` parents to
            // `sel % (i + 1)` (0 = root, k = heading k-1) — well-founded by
            // construction since only earlier headings are eligible.
            let parents: Vec<EntityUri> = headings
                .iter()
                .enumerate()
                .map(
                    |(i, (_, _, _, _, parent_sel))| match *parent_sel as usize % (i + 1) {
                        0 => doc_uri.clone(),
                        k => EntityUri::block(&ids[k - 1]),
                    },
                )
                .collect();
            let blocks: Vec<Block> = headings
                .into_iter()
                .enumerate()
                .map(|(i, (headline, _, make_task, make_requires, _))| {
                    let mut b =
                        Block::new_text(EntityUri::block(&ids[i]), parents[i].clone(), &headline);
                    b.set_property("ID", Value::String(ids[i].clone()));
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
                    // the parsed order.
                    // Stored as a `block:` URI to match the parser boundary.
                    if make_requires && i > 0 && parents[i] == parents[i - 1] {
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
            "list(#{item_template: row(state_toggle(col(\"task_state\")), \
             editable_text(col(\"content\")))})",
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

    // Tree view with virtual child — exercises the creation-slot path.
    // `creation_slot: true` triggers `interpret_virtual_child` (static /
    // snapshot path); `virtual_parent` is deliberately omitted — the builder
    // falls back to `ba.ctx.row().get("id")` (the focused block's id).
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
            "tree(#{parent_id: col(\"parent_id\"), sortkey: col(\"sequence\"), item_template: \
             render_entity(), creation_slot: true})",
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

    // Render-artifact file: a heading carrying BOTH a `Source` and an `Image`
    // child PLUS a `Text` sub-heading — the only shape that puts a mixed
    // `Source`/`Image`/`Text` sibling group under one parent. Image blocks are
    // NOT reachable via the interactive create path (`BlockContent` has no
    // Image variant — a `block create` always yields Text or Source); they only
    // arise from the parser's `[[file:…]]` links, so the org-file path is the
    // one faithful producer.
    //
    // The renderer emits section content (Source/Image, group 0) ahead of the
    // sub-heading and preserves this vec's order WITHIN group 0, but the parser
    // re-emits all Sources before all Images regardless — so the stored order is
    // always `Source < Image < Text` (world (b)). `image_first` deliberately
    // varies the on-disk order of the two artifacts so the reference model's
    // sibling-order prediction is exercised in both directions; the presence of
    // the non-exempt Text sub-heading is what makes the ordering actually
    // checked (a pure Source/Image group is order-exempt).
    let file_with_render_artifacts = (
        "[a-z_]+_[0-9]+\\.org",
        "[A-Z][a-zA-Z0-9 ]{0,15}", // heading
        "[a-z0-9-]+",              // heading id
        "[A-Z][a-zA-Z0-9 ]{0,15}", // sub-heading
        "[a-z0-9-]+",              // sub-heading id
        prop::sample::select(vec!["python", "rust", "shell"]),
        "[a-zA-Z_][a-zA-Z0-9_ ]{3,20}", // source body
        prop::sample::select(vec!["png", "jpg", "gif", "webp", "svg"]),
        "[a-z][a-z0-9_]{2,12}", // image stem
        prop::bool::ANY,        // image_first
    )
        .prop_map(
            |(
                filename,
                headline,
                id,
                sub_headline,
                sub_id_raw,
                lang,
                body,
                ext,
                stem,
                image_first,
            )| {
                let doc_uri = EntityUri::block("gen-placeholder");
                let heading_uri = EntityUri::block(&id);
                // Sub-heading id must differ from the heading id (org `:ID:`
                // uniqueness); suffix on collision.
                let sub_id = if sub_id_raw == id {
                    format!("{sub_id_raw}-sub")
                } else {
                    sub_id_raw
                };

                let mut heading = Block::new_text(heading_uri.clone(), doc_uri.clone(), &headline);
                heading.set_property("ID", Value::String(id.clone()));

                // Parser-faithful ids: `{parent}::src::0` / `{parent}::img::0`.
                let source = Block::new_source(
                    EntityUri::block(&format!("{id}::src::0")),
                    heading_uri.clone(),
                    lang,
                    &body,
                );
                let image = Block::new_image(
                    EntityUri::block(&format!("{id}::img::0")),
                    heading_uri.clone(),
                    format!("attachments/{stem}.{ext}"),
                );

                let mut sub =
                    Block::new_text(EntityUri::block(&sub_id), heading_uri, &sub_headline);
                sub.set_property("ID", Value::String(sub_id.clone()));

                // Vec order == on-disk document order. `image_first` flips the
                // Source/Image order the renderer writes; the sub-heading trails
                // (the renderer hoists section content ahead of it regardless).
                let mut blocks = vec![heading];
                if image_first {
                    blocks.push(image);
                    blocks.push(source);
                } else {
                    blocks.push(source);
                    blocks.push(image);
                }
                blocks.push(sub);
                (filename, blocks)
            },
        )
        .boxed();

    // Advice-rule file: a headline plus ONE `holon_advice_rule_yaml` source
    // block (ADR 0022). Structurally mirrors `file_with_profile` — a source
    // block under a heading — but the source language marks it a runtime rule
    // definition that the engine synthesizes into a matview. The slug is fixed
    // (`pbt_lessons`): at most one minted rule exists per run, so one slug is
    // enough — but it must NOT be `lessons_for_tasks`: the bundled INACTIVE
    // seed rule owns that slug, and the reconciler's first-owner-wins arbiter
    // refuses a same-slug newcomer (SlugCollision) even while the owner is
    // inactive — the minted ACTIVE rule would never synthesize a matview and
    // the advice invariants would sit in a permanent false RED.
    // Anchor/candidate tags are drawn DISTINCT from `ADVICE_TAG_POOL`
    // (the same pool `SetEdgeField` tags blocks from) so overlaps — hence
    // nonempty advice — are reachable. `k` is kept small (1..=3) so top-K
    // truncation cases are reachable; `active` is weighted ~4:1 true.
    let file_with_advice_rule = (
        "[a-z_]+_[0-9]+\\.org",
        "[A-Z][a-zA-Z0-9 ]{0,15}",
        "[a-z0-9-]+",
        prop::sample::select(ADVICE_TAG_POOL.to_vec()),
        prop::sample::select(ADVICE_TAG_POOL.to_vec()),
        1..=3u8,
        prop::sample::select(vec![true, true, true, true, false]),
    )
        .prop_filter(
            "advice anchor and candidate source tags must differ",
            |(_, _, _, anchor_tag, source_tag, _, _)| anchor_tag != source_tag,
        )
        .prop_map(
            |(filename, headline, id, anchor_tag, source_tag, k, active)| {
                let doc_uri = EntityUri::block("gen-placeholder");
                let heading_uri = EntityUri::block(&id);

                let mut heading = Block::new_text(heading_uri.clone(), doc_uri.clone(), &headline);
                heading.set_property("ID", Value::String(id.clone()));

                // Shaped like `crates/holon-advice/assets/lessons_for_tasks.yaml`.
                let yaml = format!(
                    "name: pbt_lessons\nactive: {active}\nanchor:\n  has_tag: \
                     {anchor_tag}\ncandidates:\n  tag_overlap_recency:\n    source:\n      \
                     has_tag: {source_tag}\nk: {k}\n"
                );

                // Round-trip guard (fail loud): the oracle reuses this exact prod
                // parser, so a mismatch here would be parser-circular. Assert the
                // parsed rule equals the typed intent before it leaves the arm.
                let parsed = holon_advice::parse_advice_rule(&yaml)
                    .expect("generator-minted advice rule must parse");
                assert_eq!(
                    parsed.anchor,
                    holon_advice::AnchorSelector::HasTag(anchor_tag.to_string()),
                );
                let holon_advice::ScoringTemplate::TagOverlapRecency(spec) = &parsed.candidates;
                assert_eq!(
                    spec.source,
                    holon_advice::AnchorSelector::HasTag(source_tag.to_string()),
                );
                assert_eq!(parsed.k.get(), k);
                assert_eq!(parsed.active, active);

                let mut rule_block = Block::new_source(
                    EntityUri::block(&format!("{id}::src::0")),
                    heading_uri,
                    "holon_advice_rule_yaml",
                    &yaml,
                );
                rule_block.set_sequence(1);

                let blocks = vec![heading, rule_block];
                (filename, blocks)
            },
        )
        .boxed();

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
            // Uniquify mount + child ids together (org files require unique
            // `:ID:`s; a random collision between the mount and a child, or
            // between two children, is an illegal input state).
            let ids = uniquify_ids(
                std::iter::once(mount_id).chain(children.iter().map(|(_, id)| id.clone())),
            );
            let mount_uri = EntityUri::block(&ids[0]);

            let mut mount = Block::new_text(mount_uri.clone(), doc_uri.clone(), &mount_headline);
            mount.set_property("ID", Value::String(ids[0].clone()));
            mount.set_property("share-role", Value::String("mount".to_string()));
            mount.set_property("shared-tree-id", Value::String(tree_id.clone()));

            let mut blocks = vec![mount];
            for ((headline, _), child_id) in children.iter().zip(&ids[1..]) {
                let mut child =
                    Block::new_text(EntityUri::block(child_id), mount_uri.clone(), headline);
                child.set_property("ID", Value::String(child_id.clone()));
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

    // Mix the advice-rule arm into a profile-bearing base at a weight
    // comparable to `file_with_profile`'s (roughly 1 part in ~10). The arm is
    // only present when `allow_advice_rule` holds — i.e. no advice-rule block
    // exists in the reference yet — so at most one rule is ever minted per run
    // (the `active_rule` ≤1 invariant). The gate is re-checked under shrinking
    // in `WriteOrgFile::preconditions`.
    let mix_advice = |base: BoxedStrategy<(String, Vec<Block>)>,
                      advice: BoxedStrategy<(String, Vec<Block>)>| {
        if allow_advice_rule {
            prop_oneof![9 => base, 1 => advice].boxed()
        } else {
            let _ = advice; // hold onto the strategy without firing
            base
        }
    };

    if allow_index_override {
        let base = if mount_enabled {
            prop_oneof![
                3 => regular_file,
                2 => index_file_prql,
                1 => index_file_gql,
                1 => index_file_gql_varlen,
                1 => index_file_sql,
                1 => index_file_tree,
                1 => file_with_profile,
                1 => file_with_render_artifacts,
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
                1 => file_with_render_artifacts,
            ]
            .boxed()
        };
        mix_advice(base, file_with_advice_rule)
    } else if extended_gen_enabled() {
        // Extended-gen axis 4: profile-bearing files in the no-overrides
        // configuration. The test profile YAMLs render just
        // `row(editable_text(...))` — no state_toggle — so the ref model
        // must track the active profile per rendered block. Triaged on the
        // extended slice before promotion.
        let base = prop_oneof![
            8 => regular_file,
            1 => file_with_profile,
            1 => file_with_render_artifacts,
        ]
        .boxed();
        mix_advice(base, file_with_advice_rule)
    } else {
        // Profile-bearing files override the default block entity profile,
        // and the test's profile YAMLs render just `row(editable_text(...))`
        // — no state_toggle. We keep them out of the no-overrides
        // configuration so the default `assets/default/types/block_profile.yaml`
        // (which has the state_toggle variant) stays in effect.
        // Advice-rule blocks do NOT override profiles (inert data until the
        // weave lands, then the feature under test) — so unlike profiles they
        // stay in the default mix; excluding them made advice unreachable.
        // The render-artifact arm rides along here too so the mixed
        // Source/Image/Text sibling group is reachable in the default config.
        let base = prop_oneof![
            8 => regular_file,
            1 => file_with_render_artifacts,
        ]
        .boxed();
        mix_advice(base, file_with_advice_rule)
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
                id,
                fields: [(prop_name.to_string(), Value::String(prop_value))]
                    .into_iter()
                    .collect(),
            });

        prop_oneof![2 => update_content, 1 => update_custom_prop].boxed()
    };

    let delete = prop::sample::select(ids).prop_map(|id| Mutation::Delete { id });

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
            id,
            fields: [("content".to_string(), Value::String(yaml))]
                .into_iter()
                .collect(),
        }
    })
}

#[cfg(test)]
mod advice_rule_tests {
    use holon_advice::AnchorSelector;
    use holon_advice::ScoringTemplate;
    use holon_advice::parse_advice_rule;

    use super::*;

    /// Reachability floor for the advice-rule arm: with `allow_advice_rule`
    /// the default (no-overrides) file mix must actually draw
    /// `holon_advice_rule_yaml` source blocks at roughly its 1-in-10 weight —
    /// a silent gate-out here makes both advice keystone invariants vacuous.
    #[test]
    fn advice_rule_arm_reachable_in_default_mix() {
        use proptest::strategy::ValueTree;
        use proptest::test_runner::TestRunner;
        let mut runner = TestRunner::deterministic();
        let strat = generate_org_file_content_with_keywords(None, false, true);
        let hits = (0..200)
            .filter(|_| {
                let (_, blocks) = strat
                    .new_tree(&mut runner)
                    .expect("file strategy must draw")
                    .current();
                blocks.iter().any(|b| {
                    b.source_language
                        .as_ref()
                        .map(|sl| sl.to_string())
                        .as_deref()
                        == Some(holon_advice::ADVICE_RULE_SOURCE_LANGUAGE)
                })
            })
            .count();
        assert!(
            hits > 5,
            "advice-rule arm near-vacuous in the default file mix: {hits}/200 draws carried a \
             holon_advice_rule_yaml block (expected ~20)"
        );
    }

    /// Stage-2 reachability floor for the advice weave (F3). Stage 1
    /// (`advice_rule_arm_reachable_in_default_mix`) only proves a RULE BLOCK is
    /// drawn. What makes BOTH advice keystone invariants non-vacuous is a
    /// non-empty `AdviceExpectation.scored` — a tag COLLISION between the
    /// rule's anchor/source tags and the tags `SetEdgeField` lands on
    /// blocks. Nothing floored that collision, so a `SetEdgeField` weight
    /// change or an `ADVICE_TAG_POOL` retune could silently make `scored`
    /// always empty (the empty-empty anchor is skipped, so
    /// `check_advice_relation` is never exercised) while the stage-1
    /// rule-presence floor stays green.
    ///
    /// This floors the collision over the SAME generator space both sides draw
    /// from: rule anchor/source tags (distinct, from `ADVICE_TAG_POOL`, as
    /// `file_with_advice_rule` draws) + per-block tag sets
    /// (`subsequence(ADVICE_TAG_POOL, 1..=3)`, as the `SetEdgeField` pool
    /// sub-arm draws). A non-empty `scored` — computed by the SAME
    /// `expectation_for` the oracle uses — must be reachable at non-trivial
    /// frequency.
    #[test]
    fn advice_scored_nonempty_reachable_in_default_mix() {
        use proptest::strategy::Strategy;
        use proptest::strategy::ValueTree;
        use proptest::test_runner::TestRunner;

        use crate::pbt::advice_expectation::expectation_for;

        let mut runner = TestRunner::deterministic();
        let scenario = (
            // Rule tags: distinct pool tags + small k (the `file_with_advice_rule` draw).
            (
                prop::sample::select(ADVICE_TAG_POOL.to_vec()),
                prop::sample::select(ADVICE_TAG_POOL.to_vec()),
                1..=3u8,
            )
                .prop_filter("anchor and source tags must differ", |(a, s, _)| a != s),
            // Per-block tag sets: the `SetEdgeField` pool sub-arm draw. Four
            // blocks (one anchor + three candidates) is enough for a collision.
            proptest::collection::vec(
                proptest::sample::subsequence(ADVICE_TAG_POOL.to_vec(), 1..=3),
                4,
            ),
        );

        let root = EntityUri::parse("block:root").expect("root uri");
        let hits = (0..400)
            .filter(|_| {
                let ((anchor_tag, source_tag, k), tag_sets) = scenario
                    .new_tree(&mut runner)
                    .expect("advice scenario strategy must draw")
                    .current();
                let yaml = format!(
                    "name: pbt_lessons\nactive: true\nanchor:\n  has_tag: {anchor_tag}\n\
                     candidates:\n  tag_overlap_recency:\n    source:\n      has_tag: \
                     {source_tag}\nk: {k}\n"
                );
                let rule =
                    holon_advice::parse_advice_rule(&yaml).expect("generated advice rule parses");
                let blocks: std::collections::BTreeMap<EntityUri, Block> = tag_sets
                    .into_iter()
                    .enumerate()
                    .map(|(i, tags)| {
                        let id = EntityUri::parse(&format!("block:b{i}")).expect("block uri");
                        let mut b = Block::new_text(id.clone(), root.clone(), String::new());
                        b.tags = holon_api::Tags::from_tag_iter(tags.into_iter().map(String::from));
                        (id, b)
                    })
                    .collect();
                blocks
                    .keys()
                    .any(|aid| !expectation_for(&blocks, &rule, aid).scored.is_empty())
            })
            .count();

        assert!(
            hits > 20,
            "advice tag-collision near-vacuous: only {hits}/400 draws produced a non-empty \
             `scored` — the rule tags and the SetEdgeField pool tags no longer overlap, so both \
             advice invariants would run vacuously (a pool/weight retune vacated the weave)"
        );
    }

    proptest! {
        /// Pins the advice-rule YAML template shut: any draw over the same input
        /// space the `file_with_advice_rule` arm uses (distinct pool tags, small
        /// k, weighted-active flag) must parse back to the typed intent. This is
        /// the same round-trip the arm asserts inline — surfaced as a standalone
        /// test so a template regression fails here, not only mid-generation.
        #[test]
        fn advice_rule_yaml_round_trips(
            (anchor_tag, source_tag, k, active) in (
                prop::sample::select(ADVICE_TAG_POOL.to_vec()),
                prop::sample::select(ADVICE_TAG_POOL.to_vec()),
                1..=3u8,
                prop::sample::select(vec![true, true, true, true, false]),
            )
                .prop_filter("anchor and source tags must differ", |(a, s, _, _)| a != s)
        ) {
            let yaml = format!(
                "name: pbt_lessons\n\
                 active: {active}\n\
                 anchor:\n  has_tag: {anchor_tag}\n\
                 candidates:\n  tag_overlap_recency:\n    source:\n      has_tag: {source_tag}\n\
                 k: {k}\n"
            );
            let parsed = parse_advice_rule(&yaml).expect("advice rule must parse");
            prop_assert_eq!(&parsed.anchor, &AnchorSelector::HasTag(anchor_tag.to_string()));
            let ScoringTemplate::TagOverlapRecency(spec) = &parsed.candidates;
            prop_assert_eq!(&spec.source, &AnchorSelector::HasTag(source_tag.to_string()));
            prop_assert_eq!(parsed.k.get(), k);
            prop_assert_eq!(parsed.active, active);
        }
    }
}
