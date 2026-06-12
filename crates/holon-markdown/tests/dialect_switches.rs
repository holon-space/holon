//! Behavioral contract for [`MarkdownDialect`]: every feature is an atomic,
//! orthogonal switch.
//!
//! The headline guarantee — *flipping one switch does not change another
//! feature's behavior* — is proven by [`sidecar_switches_are_orthogonal`]:
//! it parses one document with every switch on, then flips each sidecar
//! switch off in turn and asserts that only that feature's property changes
//! while the block content and every other property stay byte-identical.

use holon_api::block::Block;
use holon_api::types::StateCategory;
use holon_api::EntityUri;
use holon_core::file_format::{FileFormatAdapter, FileFormatParseResult};
use holon_markdown::dialect::{TaskKeywords, TaskMarker};
use holon_markdown::{
    parse_markdown_file, DailyNoteConfig, MarkdownDialect, MarkdownFormatAdapter, ParseResult,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn parse_at(rel_path: &str, content: &str, dialect: &MarkdownDialect) -> ParseResult {
    let root = PathBuf::from("/vault");
    let path = root.join(rel_path);
    parse_markdown_file(&path, content, &EntityUri::no_parent(), &root, dialect).unwrap()
}

fn parse_with(content: &str, dialect: &MarkdownDialect) -> ParseResult {
    parse_at("Note.md", content, dialect)
}

fn prop<'a>(block: &'a Block, key: &str) -> Option<&'a str> {
    block.properties.get(key).and_then(|v| v.as_string())
}

/// Sidecar properties the seven content-preserving switches own, one each.
const SIDECAR_KEYS: &[&str] = &[
    "wikilinks",
    "embeds",
    "self_links",
    "inline_tags",
    "highlights",
    "comments",
    "callouts",
];

fn sidecar_props(block: &Block) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for key in SIDECAR_KEYS {
        if let Some(value) = prop(block, key) {
            out.insert((*key).to_string(), value.to_string());
        }
    }
    out
}

/// One heading whose body exercises every inline feature in a distinct,
/// non-overlapping span.
const RICH: &str = "\
# Topic ^h1

Body with [[CrossLink]], ![[Pic.png]], [[#Section]], #project, #area/leaf, ==hot==, %%note%%.

> [!tip] Callout title
> inner line
";

// ---------------------------------------------------------------------------
// Presets
// ---------------------------------------------------------------------------

#[test]
fn obsidian_baseline_has_every_sidecar() {
    let r = parse_with(RICH, &MarkdownDialect::obsidian());
    let props = sidecar_props(&r.blocks[0]);
    for key in SIDECAR_KEYS {
        assert!(props.contains_key(*key), "obsidian preset missing {key}");
    }
    assert_eq!(prop(&r.blocks[0], "wikilinks"), Some(r#"["CrossLink"]"#));
    assert_eq!(prop(&r.blocks[0], "embeds"), Some(r#"["Pic.png"]"#));
    assert_eq!(prop(&r.blocks[0], "self_links"), Some(r##"["#Section"]"##));
    assert_eq!(prop(&r.blocks[0], "inline_tags"), Some(r#"["project","area/leaf"]"#));
    assert_eq!(prop(&r.blocks[0], "highlights"), Some(r#"["hot"]"#));
    assert_eq!(prop(&r.blocks[0], "comments"), Some(r#"["note"]"#));
    assert!(prop(&r.blocks[0], "callouts").unwrap().contains("\"tip\""));
}

#[test]
fn commonmark_is_fully_inert() {
    // No frontmatter here: under CommonMark a leading `---` interacts with
    // setext-heading rules, which is the markdown parser's business, not this
    // switch's. `yaml_frontmatter_toggle` covers the frontmatter-off path.
    let content = "# [ ] Heading ^h1\n\nbody [[Link]] #tag ==hi== %%c%%\n";
    let r = parse_with(content, &MarkdownDialect::commonmark());
    // No block id stripped → fresh UUID, marker stays in content, nothing to
    // write back.
    assert_ne!(r.blocks[0].id.id(), "h1");
    assert!(r.blocks[0].content.contains("^h1"));
    assert!(r.blocks_needing_ids.is_empty());
    // No task parsed.
    assert!(prop(&r.blocks[0], "task_state").is_none());
    assert!(r.blocks[0].content.contains("[ ]"));
    // No sidecar metadata extracted at all.
    assert!(sidecar_props(&r.blocks[0]).is_empty());
}

// ---------------------------------------------------------------------------
// Orthogonality — the headline guarantee
// ---------------------------------------------------------------------------

type Toggle = fn(&mut MarkdownDialect);

#[test]
fn sidecar_switches_are_orthogonal() {
    let switches: &[(&str, Toggle)] = &[
        ("wikilinks", |d| d.wikilinks = false),
        ("embeds", |d| d.embeds = false),
        ("self_links", |d| d.self_links = false),
        ("inline_tags", |d| d.inline_tags = false),
        ("highlights", |d| d.highlights = false),
        ("comments", |d| d.comments = false),
        ("callouts", |d| d.callouts = false),
    ];

    let baseline = parse_with(RICH, &MarkdownDialect::obsidian());
    let base_props = sidecar_props(&baseline.blocks[0]);
    let base_content = baseline.blocks[0].content.clone();

    for (owned, toggle) in switches {
        let mut dialect = MarkdownDialect::obsidian();
        toggle(&mut dialect);
        let r = parse_with(RICH, &dialect);
        let props = sidecar_props(&r.blocks[0]);

        // Content is never touched by a sidecar switch.
        assert_eq!(
            r.blocks[0].content, base_content,
            "flipping {owned} altered block content"
        );
        // Exactly the owned property disappears; every other is unchanged.
        assert!(
            !props.contains_key(*owned),
            "{owned}=false still produced its property"
        );
        for (key, value) in &base_props {
            if key == owned {
                continue;
            }
            assert_eq!(
                props.get(key),
                Some(value),
                "flipping {owned} changed unrelated property {key}"
            );
        }
    }
}

#[test]
fn nested_tags_only_refines_inline_tags() {
    let mut dialect = MarkdownDialect::obsidian();
    dialect.nested_tags = false;

    let with_nesting = parse_with(RICH, &MarkdownDialect::obsidian());
    let without_nesting = parse_with(RICH, &dialect);

    // The inline_tags value changes...
    assert_eq!(prop(&with_nesting.blocks[0], "inline_tags"), Some(r#"["project","area/leaf"]"#));
    assert_eq!(prop(&without_nesting.blocks[0], "inline_tags"), Some(r#"["project","area"]"#));

    // ...but nothing else does.
    for key in SIDECAR_KEYS {
        if *key == "inline_tags" {
            continue;
        }
        assert_eq!(
            prop(&with_nesting.blocks[0], key),
            prop(&without_nesting.blocks[0], key),
            "nested_tags changed unrelated property {key}"
        );
    }
}

// ---------------------------------------------------------------------------
// Content-transforming switches, individually
// ---------------------------------------------------------------------------

#[test]
fn block_ids_toggle() {
    let on = parse_with("# Heading ^abc\n", &MarkdownDialect::obsidian());
    assert_eq!(on.blocks[0].id.id(), "abc");
    assert!(!on.blocks[0].content.contains("^abc"));
    assert!(on.blocks_needing_ids.is_empty());

    let mut off_dialect = MarkdownDialect::obsidian();
    off_dialect.block_ids = false;
    let off = parse_with("# Heading ^abc\n", &off_dialect);
    assert_ne!(off.blocks[0].id.id(), "abc");
    assert!(off.blocks[0].content.contains("^abc"));
    assert!(off.blocks_needing_ids.is_empty());
}

#[test]
fn gfm_tasks_toggle() {
    let on = parse_with("# [x] done thing ^t1\n", &MarkdownDialect::obsidian());
    assert_eq!(prop(&on.blocks[0], "task_state"), Some("DONE"));
    assert_eq!(prop(&on.blocks[0], "task_state_kind"), Some("done"));
    assert!(on.blocks[0].content.starts_with("done thing"));

    let mut off_dialect = MarkdownDialect::obsidian();
    off_dialect.gfm_tasks = false;
    let off = parse_with("# [x] done thing ^t1\n", &off_dialect);
    assert!(prop(&off.blocks[0], "task_state").is_none());
    assert!(off.blocks[0].content.starts_with("[x] done thing"));
}

#[test]
fn custom_task_keywords_are_table_driven() {
    let mut dialect = MarkdownDialect::obsidian();
    dialect.task_keywords = TaskKeywords {
        markers: vec![TaskMarker {
            marker: ">".into(),
            keyword: "LATER".into(),
            category: StateCategory::Active,
        }],
    };

    let custom = parse_with("# [>] deferred ^a\n", &dialect);
    assert_eq!(prop(&custom.blocks[0], "task_state"), Some("LATER"));

    // A marker absent from the table is not a task under this dialect.
    let plain = parse_with("# [ ] todo ^b\n", &dialect);
    assert!(prop(&plain.blocks[0], "task_state").is_none());
    assert!(plain.blocks[0].content.starts_with("[ ] todo"));
}

#[test]
fn yaml_frontmatter_toggle() {
    let content = "---\ntitle: Hello\ntags: [a, b]\n---\n# H ^h\n";

    let on = parse_with(content, &MarkdownDialect::obsidian());
    assert_eq!(prop(&on.document, "title"), Some("Hello"));

    let mut off_dialect = MarkdownDialect::obsidian();
    off_dialect.yaml_frontmatter = false;
    let off = parse_with(content, &off_dialect);
    // No frontmatter parsed → no projected metadata.
    assert!(!off.document.properties.contains_key("title"));
    // The yaml text is not dropped — it survives as ordinary markdown content
    // (exactly where depends on CommonMark's `---`/setext handling).
    let all_text: String = std::iter::once(off.document.content.clone())
        .chain(off.blocks.iter().map(|b| b.content.clone()))
        .collect();
    assert!(all_text.contains("title: Hello"));
}

#[test]
fn frontmatter_aliases_toggle_moves_storage_but_both_round_trip() {
    let content = "---\naliases: [Foo, F]\n---\n# H ^h\n";

    let on = parse_with(content, &MarkdownDialect::obsidian());
    assert_eq!(prop(&on.document, "aliases"), Some(r#"["Foo","F"]"#));
    let extra_on = prop(&on.document, "frontmatter_extra").unwrap_or("");
    assert!(!extra_on.contains("aliases"));

    let mut off_dialect = MarkdownDialect::obsidian();
    off_dialect.frontmatter_aliases = false;
    let off = parse_with(content, &off_dialect);
    assert!(!off.document.properties.contains_key("aliases"));
    assert!(prop(&off.document, "frontmatter_extra").unwrap().contains("aliases"));
}

// ---------------------------------------------------------------------------
// Specific features
// ---------------------------------------------------------------------------

#[test]
fn canonical_block_ref_is_a_cross_file_link() {
    let r = parse_with("# H ^h\n\nsee [[Note#^para-2]] here\n", &MarkdownDialect::obsidian());
    assert_eq!(prop(&r.blocks[0], "wikilinks"), Some(r#"["Note"]"#));
}

#[test]
fn daily_notes_inferred_only_when_configured_and_matching() {
    let mut dialect = MarkdownDialect::obsidian();
    dialect.daily_notes = Some(DailyNoteConfig {
        folder: Some("Daily".to_string()),
        format: "%Y-%m-%d".to_string(),
    });

    let hit = parse_at("Daily/2026-06-18.md", "# Today ^t\n", &dialect);
    assert!(matches!(
        hit.document.properties.get("daily_note"),
        Some(holon_api::Value::Boolean(true))
    ));
    assert_eq!(prop(&hit.document, "date"), Some("2026-06-18"));

    // Right name, wrong folder.
    let wrong_folder = parse_at("Notes/2026-06-18.md", "# x ^t\n", &dialect);
    assert!(!wrong_folder.document.properties.contains_key("daily_note"));

    // Right folder, non-date name.
    let non_date = parse_at("Daily/Inbox.md", "# x ^t\n", &dialect);
    assert!(!non_date.document.properties.contains_key("daily_note"));

    // Feature off by default.
    let off = parse_at("Daily/2026-06-18.md", "# x ^t\n", &MarkdownDialect::obsidian());
    assert!(!off.document.properties.contains_key("daily_note"));
}

// ---------------------------------------------------------------------------
// Round-trip fidelity
// ---------------------------------------------------------------------------

fn round_trip(
    adapter: &MarkdownFormatAdapter,
    content: &str,
) -> (String, FileFormatParseResult, FileFormatParseResult) {
    let path = Path::new("/vault/Note.md");
    let root = Path::new("/vault");
    let parent = EntityUri::no_parent();
    let first = adapter.parse(path, content, &parent, root).unwrap();
    let rendered = adapter.render_document(&first.document, &first.blocks, path, &first.document.id);
    let second = adapter.parse(path, &rendered, &parent, root).unwrap();
    (rendered, first, second)
}

#[test]
fn commonmark_round_trip_emits_no_obsidian_syntax() {
    let adapter = MarkdownFormatAdapter::commonmark();
    let (rendered, first, second) = round_trip(&adapter, "# Heading\n\nplain body\n");
    assert_eq!(first.blocks.len(), second.blocks.len());
    assert!(!rendered.starts_with("---"), "frontmatter leaked: {rendered}");
    assert!(!rendered.contains('^'), "block-id marker leaked: {rendered}");
}

#[test]
fn obsidian_round_trip_preserves_inline_features() {
    let adapter = MarkdownFormatAdapter::obsidian();
    let (_, first, second) = round_trip(&adapter, RICH);
    // Structure and identity are stable (matching the existing round-trip
    // tests' fidelity level).
    assert_eq!(first.blocks.len(), second.blocks.len());
    assert_eq!(first.blocks[0].id.id(), second.blocks[0].id.id());
    // Every feature's sidecar metadata is re-derived identically...
    assert_eq!(
        sidecar_props(&first.blocks[0]),
        sidecar_props(&second.blocks[0])
    );
    // ...and the raw spans survive verbatim in content.
    for marker in ["[[CrossLink]]", "![[Pic.png]]", "==hot==", "%%note%%", "[!tip]"] {
        assert!(
            second.blocks[0].content.contains(marker),
            "{marker} did not survive the round-trip"
        );
    }
}
