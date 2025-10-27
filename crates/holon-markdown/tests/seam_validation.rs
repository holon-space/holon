//! Phase 1 seam validation: drive `MarkdownFormatAdapter` through the
//! same `dyn FileFormatAdapter` boundary that `OrgFormatAdapter` lives
//! behind. The point isn't to test markdown specifics (that's the unit
//! suite) — it's to prove the trait surface is a real seam by hosting two
//! impls without touching the trait.

use holon_api::types::ContentType;
use holon_api::EntityUri;
use holon_core::file_format::FileFormatAdapter;
use holon_markdown::MarkdownFormatAdapter;
use std::path::PathBuf;
use std::sync::Arc;

#[test]
fn dyn_dispatch_through_trait_object_round_trips_obsidian_vault_file() {
    let adapter: Arc<dyn FileFormatAdapter> = Arc::new(MarkdownFormatAdapter::new());
    let path = PathBuf::from("/vault/projects/launch.md");
    let root = PathBuf::from("/vault");
    let parent = EntityUri::no_parent();
    let original = "---\n\
        title: Launch Plan\n\
        tags: [project, urgent]\n\
        ---\n\
        \n\
        intro paragraph mentions [[Other Note]].\n\
        \n\
        # Goals ^aa\n\
        \n\
        - what we want\n\
        - by when\n\
        \n\
        ```holon_prql\n\
        from block | filter task_state == 'TODO'\n\
        ```\n\
        \n\
        ## Subgoal ^bb\n\
        \n\
        details.\n";

    let parsed = adapter.parse(&path, original, &parent, &root).unwrap();
    assert!(parsed.document.is_page());
    assert_eq!(parsed.document.title(), "launch");
    assert!(parsed.document.content.contains("[[Other Note]]"));

    let goals = parsed
        .blocks
        .iter()
        .find(|b| b.id.id() == "aa")
        .expect("goals heading present");
    let subgoal = parsed
        .blocks
        .iter()
        .find(|b| b.id.id() == "bb")
        .expect("subgoal heading present");
    let source = parsed
        .blocks
        .iter()
        .find(|b| matches!(b.content_type, ContentType::Source))
        .expect("source child present");

    assert_eq!(subgoal.parent_id, goals.id);
    assert_eq!(source.parent_id, goals.id);
    assert_eq!(
        source
            .source_language
            .as_ref()
            .map(|l| l.to_string())
            .as_deref(),
        Some("holon_prql")
    );

    let rendered =
        adapter.render_document(&parsed.document, &parsed.blocks, &path, &parsed.document.id);
    assert!(rendered.starts_with("---\n"));
    assert!(rendered.contains("title: Launch Plan"));
    assert!(rendered.contains("# Goals ^aa"));
    assert!(rendered.contains("## Subgoal ^bb"));
    assert!(rendered.contains("```holon_prql"));

    let reparsed = adapter.parse(&path, &rendered, &parent, &root).unwrap();
    assert!(
        reparsed.blocks_needing_ids.is_empty(),
        "stable IDs survive a round-trip"
    );
    let same_goals = reparsed
        .blocks
        .iter()
        .find(|b| b.id.id() == "aa")
        .expect("goals survives round-trip");
    let same_subgoal = reparsed
        .blocks
        .iter()
        .find(|b| b.id.id() == "bb")
        .expect("subgoal survives round-trip");
    assert_eq!(same_subgoal.parent_id, same_goals.id);
}

#[test]
fn extensions_advertise_md_and_markdown() {
    let adapter: Arc<dyn FileFormatAdapter> = Arc::new(MarkdownFormatAdapter::new());
    assert_eq!(adapter.extensions(), &["md", "markdown"]);
}
