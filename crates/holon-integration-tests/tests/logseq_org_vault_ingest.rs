//! Directed ingest test for a REAL LogSeq `:preferred-format :org` vault
//! (ForeignVaultCompat Inc 1, docs/Proposals/ForeignVaultCompat-2026-07-12.md
//! §4-5). WS-1 already taught the org parser two LogSeq-org deltas:
//! case-insensitive `:id:` drawer lookup, and `((uuid))` block-refs →
//! `InlineMark::Link{EntityRef::Internal}`. This proves a whole small LogSeq-org
//! vault — journals + pages laid out the way LogSeq writes them — ingests into
//! the Holon block substrate with no silent loss.
//!
//! The fixture vault (`tests/fixtures/logseq_org_vault/`) is a synthesized,
//! anonymized distillation of the constructs surveyed in a real ~1k-file
//! LogSeq-org vault: `journals/YYYY_MM_DD.org` + `pages/<Title>.org` layout,
//! lowercase `:id:` drawers, cross-file `((uuid))` block-refs, bare
//! `[[Page Name]]` wiki-links (one dangling), `LATER`/`NOW`/`DONE` markers,
//! `SCHEDULED:` planning + a `:LOGBOOK:` clock drawer, and `#+title:`.
//!
//! Assertions (fail-loud, per CLAUDE.md — no silent normalization):
//!  1. NO SILENT LOSS: every `:id:`-bearing headline lands in `block_raw`.
//!  2. `:id:` IDENTITY PRESERVED (lowercase drawer): the block id is the uuid.
//!  3. `((uuid))` → `Internal` link mark, cross-file, target = `block:<uuid>`.
//!  4. bare `[[Page]]` → `Name` link mark; the DANGLING one is represented
//!     (a `Name` mark), never dropped.
//!  5. task keywords: recognized (`DONE`) parses; not-yet-recognized dialect
//!     keywords (`LATER`/`NOW`) are PRESERVED VERBATIM as content — WS-3 will
//!     map them to `task_state` (see the `#[ignore]` stub at the bottom).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::Value;
use holon_integration_tests::TestEnvironmentBuilder;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

// LogSeq lays a vault out as journals/YYYY_MM_DD.org + pages/<Title>.org. The
// fixtures live as real .org files so their exact bytes are the fixture (and to
// dodge the rustfmt escaped-string hazard).
const JOURNAL_2026_07_15: &str =
    include_str!("fixtures/logseq_org_vault/journals/2026_07_15.org");
const PAGE_PROJECT_ALPHA: &str =
    include_str!("fixtures/logseq_org_vault/pages/Project Alpha.org");
const PAGE_PROJECT_BETA: &str = include_str!("fixtures/logseq_org_vault/pages/Project Beta.org");

// Bare ids (block: scheme stripped) of every `:id:`-bearing headline in the
// fixture vault. Each MUST land in block_raw — parsed == projected.
const HEADLINE_IDS: &[&str] = &[
    "7c9e6a10-0001-4a00-9000-000000000001", // journal: [[Project Alpha]] link
    "7c9e6a10-0002-4a00-9000-000000000002", // journal: LATER + SCHEDULED + LOGBOOK
    "7c9e6a10-0003-4a00-9000-000000000003", // journal: NOW + ((uuid)) ref
    "7c9e6a10-0004-4a00-9000-000000000004", // journal: DONE
    "7c9e6a10-0005-4a00-9000-000000000005", // journal: [[Nonexistent Page]] dangling
    "7c9e6a10-1001-4a00-9000-000000000010", // page Alpha: Design
    "7c9e6a10-1002-4a00-9000-000000000011", // page Alpha: [[Project Beta]] link
    "7c9e6a10-2001-4a00-9000-00000000000d", // page Beta: [[Project Alpha]] link
];

type Row = HashMap<Arc<str>, Value>;

/// All `block_raw` rows keyed by bare id (block: prefix stripped).
async fn block_raw_by_bare_id(
    env: &holon_integration_tests::TestEnvironment,
) -> HashMap<String, Row> {
    let rows = env
        .engine()
        .execute_query(
            "SELECT id, content, marks, properties, completed FROM block_raw".to_string(),
            HashMap::new(),
            None,
        )
        .await
        .expect("query block_raw");
    rows.into_iter()
        .filter_map(|r| {
            let id = r.get("id").and_then(|v| v.as_string())?.to_string();
            let bare = id.strip_prefix("block:").unwrap_or(&id).to_string();
            Some((bare, r))
        })
        .collect()
}

fn field(row: &Row, col: &str) -> String {
    match row.get(col) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => format!("{other:?}"),
    }
}

#[test]
fn logseq_org_vault_ingests_without_loss() {
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            // LogSeq's `journals/` + `pages/` subdirectory layout — the scan is
            // recursive, so the vault ingests exactly as on disk.
            .with_org_file("journals/2026_07_15.org", JOURNAL_2026_07_15)
            .with_org_file("pages/Project Alpha.org", PAGE_PROJECT_ALPHA)
            .with_org_file("pages/Project Beta.org", PAGE_PROJECT_BETA)
            .build(rt.clone())
            .await
            .expect("boot LogSeq-org fixture vault");
        // Loro is the default authority; give ingest + marks propagation to
        // block_raw time to settle (mirrors probe_link_marks_roundtrip).
        tokio::time::sleep(Duration::from_secs(2)).await;

        let blocks = block_raw_by_bare_id(&env).await;

        // ── 1. NO SILENT LOSS ────────────────────────────────────────────
        let missing: Vec<&&str> = HEADLINE_IDS
            .iter()
            .filter(|id| !blocks.contains_key(**id))
            .collect();
        assert!(
            missing.is_empty(),
            "LogSeq-org headlines silently dropped on ingest: {missing:?}\n\
             present ids: {:?}",
            blocks.keys().collect::<Vec<_>>()
        );

        // ── 2. lowercase `:id:` IDENTITY PRESERVED ───────────────────────
        // Presence under the exact uuid (case-insensitive drawer lookup, WS-1)
        // is the proof — the id IS the drawer uuid, no minting.
        for id in HEADLINE_IDS {
            assert!(
                blocks.contains_key(*id),
                "lowercase :id: {id} not preserved as block id"
            );
        }

        // ── 3. cross-file `((uuid))` → Internal link mark ────────────────
        let now_block = &blocks["7c9e6a10-0003-4a00-9000-000000000003"];
        let now_content = field(now_block, "content");
        let now_marks = field(now_block, "marks");
        assert!(
            now_content.contains("((7c9e6a10-1001-4a00-9000-000000000010))"),
            "((uuid)) ref label not preserved in content: {now_content:?}"
        );
        assert!(
            now_marks.contains("\"type\":\"internal\"")
                && now_marks.contains("block:7c9e6a10-1001-4a00-9000-000000000010"),
            "((uuid)) block-ref did not become an Internal link mark targeting \
             the referenced block: marks={now_marks:?}"
        );

        // ── 4. bare `[[Page]]` → Name link mark; dangling represented ─────
        let alpha_link_block = &blocks["7c9e6a10-0001-4a00-9000-000000000001"];
        let alpha_marks = field(alpha_link_block, "marks");
        assert!(
            alpha_marks.contains("\"type\":\"name\"") && alpha_marks.contains("Project Alpha"),
            "bare [[Project Alpha]] wiki-link did not become a Name link mark: \
             marks={alpha_marks:?}"
        );

        let dangling_block = &blocks["7c9e6a10-0005-4a00-9000-000000000005"];
        let dangling_marks = field(dangling_block, "marks");
        assert!(
            dangling_marks.contains("\"type\":\"name\"")
                && dangling_marks.contains("Nonexistent Page"),
            "DANGLING [[Nonexistent Page]] was dropped instead of represented as \
             a Name mark: marks={dangling_marks:?}"
        );

        // ── 5. task keywords: DONE parsed, LATER/NOW preserved verbatim ───
        // DONE is a default-recognized done keyword: the keyword must survive
        // ingest somewhere (content or the task_state property) — never lost.
        let done_block = &blocks["7c9e6a10-0004-4a00-9000-000000000004"];
        let done_blob = format!(
            "{} {} {}",
            field(done_block, "content"),
            field(done_block, "properties"),
            field(done_block, "completed"),
        );
        assert!(
            done_blob.contains("DONE") || field(done_block, "completed") == "1",
            "DONE keyword lost on ingest (neither task_state nor content nor \
             completed carries it): {done_blob:?}"
        );

        // LATER / NOW are LogSeq dialect keywords, recognized since WS-3
        // landed: they must be captured as task_state — never lost, never
        // left as bare content markers.
        let later_props = field(&blocks["7c9e6a10-0002-4a00-9000-000000000002"], "properties");
        assert!(
            later_props.contains("task_state") && later_props.contains("LATER"),
            "LATER dialect keyword lost on ingest — WS-3 maps it to \
             task_state: properties={later_props:?}"
        );
        let now_props = field(&blocks["7c9e6a10-0003-4a00-9000-000000000003"], "properties");
        assert!(
            now_props.contains("task_state") && now_props.contains("NOW"),
            "NOW dialect keyword lost on ingest — WS-3 maps it to \
             task_state: properties={now_props:?}"
        );
    });
}

/// WS-3 landed: LogSeq `LATER`/`NOW` markers map to a Holon `task_state`
/// (active), exactly as `DONE`/`TODO` do — not merely survive as content.
#[test]
fn logseq_later_now_map_to_task_state() {
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_org_file("journals/2026_07_15.org", JOURNAL_2026_07_15)
            .build(rt.clone())
            .await
            .expect("boot");
        tokio::time::sleep(Duration::from_secs(2)).await;

        let blocks = block_raw_by_bare_id(&env).await;
        let later = &blocks["7c9e6a10-0002-4a00-9000-000000000002"];
        let props = field(later, "properties");
        // WS-3 target: the LATER keyword becomes an ACTIVE task_state, and the
        // content no longer carries the bare marker.
        assert!(
            props.contains("task_state") && props.contains("LATER"),
            "WS-3 not yet implemented: LATER did not map to a task_state \
             property: properties={props:?}"
        );
        assert!(
            !field(later, "content").contains("LATER"),
            "WS-3 target: once mapped, the bare LATER marker leaves content"
        );
    });
}
