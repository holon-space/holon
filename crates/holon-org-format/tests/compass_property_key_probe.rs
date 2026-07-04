//! THROWAWAY MEASUREMENT PROBE (not a gate): which org property-key forms
//! survive Holon's ingest → render round trip byte-stable?
//!
//! Reuses the write-back half of the sync loop exactly as
//! `vault_writeback_stability.rs` models it: `parse_org_file` →
//! `OrgRenderer::render_document`.

use std::path::Path;

use holon_api::EntityUri;
use holon_org_format::OrgRenderer;
use holon_org_format::parse_org_file;

const ROOT: &str = "/probe";
const FILE: &str = "/probe/compass.org";

fn write_back(source: &str) -> String {
    let path = Path::new(FILE);
    let parsed = parse_org_file(path, source, &EntityUri::no_parent(), Path::new(ROOT))
        .expect("fixture must parse");
    OrgRenderer::render_document(&parsed.document, &parsed.blocks, path, &parsed.document.id)
}

/// A single-headline document carrying exactly one drawer property.
fn fixture(key: &str, value: &str) -> String {
    format!(
        "#+TITLE: Compass probe\n\n* Probe headline\n:PROPERTIES:\n:ID: probe-block\n:{key}: \
         {value}\n:END:\n"
    )
}

#[derive(Debug)]
enum Verdict {
    Survives,
    Mangled { after: String },
    Dropped,
}

/// Locate the drawer line for `key` in `rendered` (any key spelling), by
/// looking at every `:X: Y` drawer line that is not `:ID:`/`:END:`.
fn drawer_lines(rendered: &str) -> Vec<String> {
    rendered
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| {
            l.starts_with(':')
                && l != ":PROPERTIES:"
                && l != ":END:"
                && !l.starts_with(":ID:")
                && l.matches(':').count() >= 2
        })
        .collect()
}

fn probe(key: &str, value: &str) -> (Verdict, String, bool) {
    let src = fixture(key, value);
    let out1 = write_back(&src);
    let out2 = write_back(&out1);
    let idempotent = out1 == out2;

    let expected = format!(":{key}: {value}");
    let lines = drawer_lines(&out1);
    let verdict = if lines.iter().any(|l| *l == expected) {
        Verdict::Survives
    } else if lines.is_empty() {
        Verdict::Dropped
    } else {
        Verdict::Mangled {
            after: lines.join(" | "),
        }
    };
    (verdict, out1, idempotent)
}

/// Content (headline title) round trip — the "underscored identifier gets
/// mangled" hazard lives here, not in the drawer.
fn title_probe(title: &str) -> (String, bool) {
    let src =
        format!("#+TITLE: Compass probe\n\n* {title}\n:PROPERTIES:\n:ID: probe-block\n:END:\n");
    let out1 = write_back(&src);
    let out2 = write_back(&out1);
    let got = out1
        .lines()
        .find(|l| l.starts_with("* "))
        .map(|l| l.trim_start_matches("* ").to_string())
        .unwrap_or_else(|| "<headline vanished>".to_string());
    (got, out1 == out2)
}

#[test]
fn compass_property_key_survival_matrix() {
    let cases: &[(&str, &str, &str)] = &[
        // (family, key, value)
        ("plain lower", "provenance", "explicit"),
        ("plain UPPER", "PROVENANCE", "inferred"),
        ("plain Mixed", "Provenance", "deduced"),
        ("kebab lower", "last-reviewed", "2026-08-11"),
        ("kebab UPPER", "LAST-REVIEWED", "2026-08-11"),
        ("kebab lower", "review-cadence", "P30D"),
        ("kebab lower", "leads-to", "compass-north"),
        ("kebab lower", "contributes-to", "compass-north"),
        ("kebab UPPER", "CONTRIBUTES-TO", "compass-north"),
        ("underscore UPPER", "LAST_UPDATED", "2026-08-11"),
        ("underscore lower", "last_updated", "2026-08-11"),
        ("underscore UPPER", "LEADS_TO", "compass-north"),
        ("_-prefixed lower", "_provenance", "explicit"),
        ("_-prefixed UPPER", "_PROVENANCE", "explicit"),
        ("camelCase", "reviewCadence", "P30D"),
        ("plain lower", "compass", "north"),
        ("plain UPPER", "COMPASS", "north"),
        // INTERNAL_KEYS / typed-edge controls
        ("internal control", "REQUIRES", "other-block"),
        ("internal control", "COLLAPSED", "t"),
        ("internal control", "TAGS", "compass"),
        ("internal control", "TODO", "NEXT"),
        ("internal control", "PRIORITY", "A"),
        ("internal control", "SCHEDULED", "<2026-08-11 Tue>"),
        ("internal control", "_source_results", "x"),
        // value shapes
        (
            "value: underscore",
            "provenance-note",
            "value_with_underscore",
        ),
        ("value: kebab", "provenance-note", "value-with-kebab"),
        (
            "value: org link",
            "leads-to-link",
            "[[id:compass-north][North]]",
        ),
        (
            "value: page link",
            "provenance-note",
            "[[Become fundamentally happy and egoless]]",
        ),
        ("value: sentinel", "contributes-to", "none"),
        ("value: bracket link", "source-link", "[[compass/north]]"),
        (
            "value: ISO datetime",
            "last-reviewed",
            "2026-08-11T09:30:00Z",
        ),
        (
            "value: multiword",
            "provenance",
            "inferred from three sources",
        ),
        ("value: empty", "provenance", ""),
        // markup-collision shapes (the known lossy-render family)
        ("value: org underline", "provenance", "_explicit_"),
        ("value: org bold-ish", "provenance", "__explicit__"),
        ("value: org bold", "provenance", "*explicit*"),
        ("value: org code", "provenance", "~explicit~"),
        ("key: org underline", "_provenance_", "explicit"),
        ("key: double underscore", "LAST__UPDATED", "2026-08-11"),
        ("key: trailing underscore", "last_updated_", "2026-08-11"),
        ("key: dotted", "compass.axis", "north"),
        ("key: colon-ish", "compass/axis", "north"),
        ("key: numeric suffix", "review-cadence-2", "P30D"),
    ];

    let mut rows: Vec<String> = Vec::new();
    rows.push(format!(
        "| {:<20} | {:<18} | {:<30} | {:<9} | {:<10} | {}",
        "FAMILY", "KEY", "VALUE", "VERDICT", "IDEMPOTENT", "AFTER (drawer lines)"
    ));
    rows.push(format!("|{}|", "-".repeat(120)));

    for (family, key, value) in cases {
        let (verdict, out1, idem) = probe(key, value);
        let (v, after) = match &verdict {
            Verdict::Survives => ("SURVIVES", format!(":{key}: {value}")),
            Verdict::Mangled { after } => ("MANGLED", after.clone()),
            Verdict::Dropped => ("DROPPED", "<no drawer line>".to_string()),
        };
        rows.push(format!(
            "| {:<20} | {:<18} | {:<30} | {:<9} | {:<10} | {}",
            family,
            key,
            format!("{value:?}"),
            v,
            if idem { "yes" } else { "NO" },
            after
        ));
        let _ = out1;
    }

    rows.push(String::new());
    rows.push("=== TITLE (content) round trip ===".to_string());
    rows.push(format!(
        "| {:<40} | {:<40} | {}",
        "BEFORE", "AFTER", "VERDICT"
    ));
    for title in [
        "LAST_UPDATED cadence",
        "last_updated cadence",
        "last-reviewed cadence",
        "provenance explicit",
        "_provenance explicit",
        "leads-to compass",
        "leads_to compass",
        "review_cadence and last_updated",
        "_provenance_ explicit",
        "__provenance__ explicit",
        "__default__",
        "*provenance* explicit",
        "~last_updated~ cadence",
        "[[id:compass-north][leads-to]]",
        "[[id:last_updated][LAST_UPDATED]]",
        "[[compass/north]]",
    ] {
        let (got, idem) = title_probe(title);
        let v = if got == title { "SURVIVES" } else { "MANGLED" };
        rows.push(format!(
            "| {:<40} | {:<40} | {} (idempotent: {})",
            title,
            got,
            v,
            if idem { "yes" } else { "NO" }
        ));
    }

    let report = rows.join("\n");
    println!("\n{report}\n");
    std::fs::write("/tmp/compass-probe-matrix.txt", &report).expect("write matrix");
}

/// The RECOMMENDED Compass key set, authored in canonical render form, must be
/// byte-identical after one write-back pass and after a second.
#[test]
fn recommended_compass_key_set_is_byte_stable() {
    // Keys in ALPHABETICAL order: that is what the renderer emits when
    // `_drawer_order` is absent (it is `_`-prefixed, so it never survives the
    // store), so an alphabetical template is stable on BOTH paths.
    let canonical = "#+TITLE: Compass\n* Reach steady-state ingest\n:PROPERTIES:\n:ID: \
                     compass-anchor\n:compass: north\n:contributes-to: \
                     compass-north\n:last-reviewed: 2026-08-11\n:leads-to: \
                     compass-north\n:provenance: inferred\n:review-cadence: P30D\n:END:\n** \
                     Support the anchor\n:PROPERTIES:\n:ID: compass-child\n:contributes-to: \
                     compass-anchor\n:last-reviewed: \
                     2026-08-11T09:30:00Z\n:provenance: explicit\n:review-cadence: P7D\n:END:\n";
    let pass1 = write_back(canonical);
    let pass2 = write_back(&pass1);
    let verdict = format!(
        "canonical == pass1: {}\npass1 == pass2: {}\n--- pass1 ---\n{pass1}",
        canonical == pass1,
        pass1 == pass2
    );
    println!("{verdict}");
    assert_eq!(
        canonical, pass1,
        "recommended Compass key set must be byte-stable"
    );
    assert_eq!(pass1, pass2, "write-back must be idempotent");
}

/// STORE-ORIGIN path: a Block built in memory (as the editor / an MCP writer /
/// a template generator produces it) has no parse-recorded inline-mark
/// protection. Render it, parse it back, and see what the content became.
#[test]
fn store_origin_content_and_property_round_trip() {
    use holon_api::block::Block;

    let mut rows: Vec<String> = Vec::new();
    rows.push("=== STORE-ORIGIN (Block::new_text, no parse metadata) ===".to_string());
    rows.push(format!(
        "| {:<40} | {:<40} | {}",
        "BEFORE", "AFTER", "VERDICT"
    ));

    let probe_content = |content: &str| -> String {
        let mut doc = Block::new_text(EntityUri::block("page-root"), EntityUri::no_parent(), "");
        doc.set_page(true);
        let block = Block::new_text(
            EntityUri::block("b1"),
            EntityUri::block("page-root"),
            content,
        );
        let path = Path::new(FILE);
        let org = OrgRenderer::render_document(&doc, &[block], path, &doc.id);
        let parsed = parse_org_file(path, &org, &EntityUri::no_parent(), Path::new(ROOT))
            .expect("parse own render");
        parsed
            .blocks
            .iter()
            .find(|b| b.id.as_str() == "block:b1")
            .map(|b| b.content.clone())
            .unwrap_or_else(|| "<block vanished>".to_string())
    };

    for content in [
        "LAST_UPDATED cadence",
        "last_updated cadence",
        "__default__",
        "_provenance_ explicit",
        "review_cadence and last_updated",
        "last-reviewed cadence",
        "provenance explicit",
    ] {
        let got = probe_content(content);
        rows.push(format!(
            "| {:<40} | {:<40} | {}",
            content,
            got,
            if got == content {
                "SURVIVES"
            } else {
                "MANGLED"
            }
        ));
    }

    // Store-origin PROPERTIES: keys set via set_property, never parsed.
    rows.push(String::new());
    rows.push("=== STORE-ORIGIN properties (set_property, no drawer order) ===".to_string());
    for key in [
        "provenance",
        "last-reviewed",
        "review-cadence",
        "LAST_UPDATED",
        "_provenance",
        "compass",
        "leads-to",
        "contributes-to",
    ] {
        let mut doc = Block::new_text(EntityUri::block("page-root"), EntityUri::no_parent(), "");
        doc.set_page(true);
        let mut block = Block::new_text(
            EntityUri::block("b1"),
            EntityUri::block("page-root"),
            "anchor",
        );
        block.set_property(key, holon_api::Value::String("explicit".to_string()));
        let path = Path::new(FILE);
        let org = OrgRenderer::render_document(&doc, &[block], path, &doc.id);
        let line = org
            .lines()
            .map(str::trim)
            .find(|l| l.starts_with(&format!(":{key}:")))
            .unwrap_or("<no drawer line>")
            .to_string();
        rows.push(format!(
            "| {:<18} | {:<9} | {}",
            key,
            if line.starts_with(':') {
                "SURVIVES"
            } else {
                "DROPPED"
            },
            line
        ));
    }

    let report = rows.join("\n");
    println!("\n{report}\n");
}

/// `_drawer_order` is itself a `_`-prefixed key, so it never survives the
/// store. Measure what key ORDER the renderer emits when it is absent.
#[test]
fn drawer_order_without_authored_order() {
    use holon_api::block::Block;

    let mut doc = Block::new_text(EntityUri::block("page-root"), EntityUri::no_parent(), "");
    doc.set_page(true);
    let mut block = Block::new_text(
        EntityUri::block("compass-anchor"),
        EntityUri::block("page-root"),
        "Reach steady-state ingest",
    );
    // Authored order, deliberately NOT alphabetical.
    for (k, v) in [
        ("provenance", "inferred"),
        ("last-reviewed", "2026-08-11"),
        ("review-cadence", "P30D"),
        ("leads-to", "compass-north"),
        ("contributes-to", "compass-north"),
        ("compass", "north"),
    ] {
        block.set_property(k, holon_api::Value::String(v.to_string()));
    }
    let path = Path::new(FILE);
    let org = OrgRenderer::render_document(&doc, &[block], path, &doc.id);
    println!("=== NO _drawer_order (post-store shape) ===\n{org}");
}

/// Dump one full rendered document so the report can quote real bytes.
#[test]
fn compass_full_document_render_dump() {
    let src = "#+TITLE: Compass probe\n\n* Compass anchor\n:PROPERTIES:\n:ID: \
               probe-block\n:provenance: explicit\n:last-reviewed: \
               2026-08-11\n:review-cadence: P30D\n:leads-to: \
               compass-north\n:LAST_UPDATED: 2026-08-11\n:_provenance: \
               hidden\n:END:\n";
    let out1 = write_back(src);
    let out2 = write_back(&out1);
    let dump = format!(
        "--- SOURCE ---\n{src}\n--- AFTER PASS 1 ---\n{out1}\n--- AFTER PASS 2 ---\n{out2}\n--- \
         PASS1 == PASS2: {} ---\n--- SOURCE == PASS1: {} ---\n",
        out1 == out2,
        src == out1
    );
    println!("{dump}");
    std::fs::write("/tmp/compass-probe-dump.txt", &dump).expect("write dump");
}
