//! Phase 0.1 spike: Loro Peritext mark API validation.
//!
//! Run: `cargo run --example loro_marks_spike --release 2>&1 | tee /tmp/loro-marks-spike.log`
//!
//! This spike answers the open hypotheses (H1, H2, H8, H12, H13) before we
//! commit the rich-text plan to the model. Each section asserts a specific
//! behavior and prints the observed outcome — the log becomes
//! `docs/loro_marks_findings.md`.

use loro::cursor::Side;
use loro::{ExpandType, ExportMode, LoroDoc, LoroValue, StyleConfig, StyleConfigMap};
use std::collections::HashMap;

fn header(title: &str) {
    eprintln!("\n=== {title} ===");
}

fn show_delta(text: &loro::LoroText) {
    let delta = text.to_delta();
    eprintln!("    delta: {delta:?}");
}

/// Build a fresh doc with our planned mark vocabulary configured once at
/// LoroDoc creation. This is the production shape per the plan.
fn make_doc() -> LoroDoc {
    let doc = LoroDoc::new();
    let mut cfg = StyleConfigMap::new();
    for key in [
        "bold",
        "italic",
        "code",
        "strike",
        "underline",
        "sub",
        "super",
    ] {
        cfg.insert(
            key.into(),
            StyleConfig {
                expand: ExpandType::After,
            },
        );
    }
    for key in ["link", "verbatim"] {
        cfg.insert(
            key.into(),
            StyleConfig {
                expand: ExpandType::None,
            },
        );
    }
    doc.config_text_style(cfg);
    doc
}

fn s_basic_mark_unmark_delta() {
    header("S1: mark/unmark/apply_delta/to_delta round-trip");
    let doc = make_doc();
    let text = doc.get_text("body");
    text.insert(0, "Hello, world!").unwrap();
    text.mark(0..5, "bold", true).unwrap();
    text.mark(7..12, "italic", true).unwrap();
    doc.commit();
    eprintln!("  after marks:");
    show_delta(&text);

    text.unmark(0..5, "bold").unwrap();
    doc.commit();
    eprintln!("  after unmark bold:");
    show_delta(&text);
}

fn s_config_late_text_creation() {
    header("S2 (H2): LoroText created AFTER config_text_style sees the configured expand");
    let doc = make_doc();
    let text = doc.get_text("late_text");
    text.insert(0, "abcdefg").unwrap();
    text.mark(0..3, "link", "https://x").unwrap();
    doc.commit();
    text.insert(3, "X").unwrap();
    doc.commit();
    eprintln!("  link is ExpandType::None — inserting at end-of-link should NOT extend it");
    show_delta(&text);
}

fn s_reconfigure_conflict() {
    header("S3 (H13): re-configure same key with conflicting ExpandType");
    let doc = make_doc();
    let mut cfg = StyleConfigMap::new();
    cfg.insert(
        "bold".into(),
        StyleConfig {
            expand: ExpandType::None,
        },
    );
    eprintln!("  attempting to re-configure 'bold' from After → None …");
    doc.config_text_style(cfg);
    eprintln!("  call returned (no panic)");

    let text = doc.get_text("body");
    text.insert(0, "abcdefg").unwrap();
    text.mark(0..3, "bold", true).unwrap();
    doc.commit();
    text.insert(3, "X").unwrap();
    doc.commit();
    eprintln!("  observed expand behavior on 'bold' after re-config:");
    show_delta(&text);
    eprintln!("  → if 'X' is bold, 'After' won (latched). If not, 'None' won.");
}

fn s_concurrent_bold_insert() {
    header("S4 (H8): peer A bolds [5..10], peer B inserts at 7 → B inherits Bold");
    let a = make_doc();
    let b = make_doc();
    a.set_peer_id(1).unwrap();
    b.set_peer_id(2).unwrap();
    let ta = a.get_text("body");
    ta.insert(0, "0123456789ABCDE").unwrap();
    a.commit();
    let snap = a.export(ExportMode::all_updates()).unwrap();
    b.import(&snap).unwrap();

    let ta = a.get_text("body");
    ta.mark(5..10, "bold", true).unwrap();
    a.commit();

    let tb = b.get_text("body");
    tb.insert(7, "INSERT").unwrap();
    b.commit();

    let snap_a = a.export(ExportMode::all_updates()).unwrap();
    let snap_b = b.export(ExportMode::all_updates()).unwrap();
    a.import(&snap_b).unwrap();
    b.import(&snap_a).unwrap();
    eprintln!("  merged delta from peer A:");
    show_delta(&a.get_text("body"));
    eprintln!("  merged delta from peer B:");
    show_delta(&b.get_text("body"));
}

fn s_concurrent_unmark_inside_bold() {
    header("S5: peer A bolds [5..10], peer B unmarks [7..8]");
    let a = make_doc();
    let b = make_doc();
    a.set_peer_id(1).unwrap();
    b.set_peer_id(2).unwrap();
    let ta = a.get_text("body");
    ta.insert(0, "0123456789ABCDE").unwrap();
    ta.mark(5..10, "bold", true).unwrap();
    a.commit();
    let snap = a.export(ExportMode::all_updates()).unwrap();
    b.import(&snap).unwrap();

    let tb = b.get_text("body");
    tb.unmark(7..8, "bold").unwrap();
    b.commit();
    let snap_b = b.export(ExportMode::all_updates()).unwrap();
    a.import(&snap_b).unwrap();
    eprintln!("  after merging B's unmark [7..8] into A:");
    show_delta(&a.get_text("body"));
}

fn s_concurrent_link_conflict() {
    header("S6: peer A links [5..10] to X, peer B links same range to Y");
    let a = make_doc();
    let b = make_doc();
    a.set_peer_id(1).unwrap();
    b.set_peer_id(2).unwrap();
    let ta = a.get_text("body");
    ta.insert(0, "0123456789ABCDE").unwrap();
    a.commit();
    let snap = a.export(ExportMode::all_updates()).unwrap();
    b.import(&snap).unwrap();

    let ta = a.get_text("body");
    ta.mark(5..10, "link", "URL_X").unwrap();
    a.commit();
    let tb = b.get_text("body");
    tb.mark(5..10, "link", "URL_Y").unwrap();
    b.commit();

    let snap_a = a.export(ExportMode::all_updates()).unwrap();
    let snap_b = b.export(ExportMode::all_updates()).unwrap();
    a.import(&snap_b).unwrap();
    b.import(&snap_a).unwrap();
    eprintln!("  peer A merged delta:");
    show_delta(&a.get_text("body"));
    eprintln!("  peer B merged delta:");
    show_delta(&b.get_text("body"));
    eprintln!("  → if both peers agree on the same value, LWW is deterministic.");
}

fn s_link_with_loro_map_value() {
    header("S7: Link mark with LoroValue::Map (kind/id/label structure)");
    let doc = make_doc();
    let text = doc.get_text("body");
    text.insert(0, "Click here please").unwrap();
    let mut map: HashMap<String, LoroValue> = HashMap::new();
    map.insert("kind".to_string(), "internal".into());
    map.insert("id".to_string(), "uuid-123".into());
    map.insert("label".to_string(), "here".into());
    let value: LoroValue = map.into();
    text.mark(6..10, "link", value).unwrap();
    doc.commit();
    eprintln!("  delta with structured link value:");
    show_delta(&text);
}

fn s_cursor_across_remote_insert() {
    header("S8 (H12): cursor stable across remote insert");
    let a = make_doc();
    let b = make_doc();
    a.set_peer_id(1).unwrap();
    b.set_peer_id(2).unwrap();
    let ta = a.get_text("body");
    ta.insert(0, "0123456789").unwrap();
    a.commit();
    let snap = a.export(ExportMode::all_updates()).unwrap();
    b.import(&snap).unwrap();

    let cursor = ta.get_cursor(5, Side::Middle).expect("cursor at 5");
    eprintln!("  cursor anchored at pos=5");

    let tb = b.get_text("body");
    tb.insert(0, "BBB").unwrap();
    b.commit();
    let snap_b = b.export(ExportMode::all_updates()).unwrap();
    a.import(&snap_b).unwrap();

    let pos = a.get_cursor_pos(&cursor).expect("resolve cursor");
    eprintln!(
        "  after remote insert of 3 chars at pos 0, cursor resolves to pos={}",
        pos.current.pos
    );
    eprintln!("  → expected: pos shifted to 8 (5 + 3 inserted before)");
}

fn s_cursor_across_mark_only() {
    header("S9 (H12): cursor stable across mark-only change (no text edit)");
    let doc = make_doc();
    let text = doc.get_text("body");
    text.insert(0, "0123456789").unwrap();
    doc.commit();
    let cursor = text.get_cursor(5, Side::Middle).expect("cursor at 5");
    text.mark(2..7, "bold", true).unwrap();
    doc.commit();
    let pos = doc.get_cursor_pos(&cursor).expect("resolve cursor");
    eprintln!(
        "  after mark applied (no text change), cursor pos={}",
        pos.current.pos
    );
    eprintln!("  → expected: still 5 (marks don't move characters)");
}

fn s_indexing_flavors() {
    header("S10: mark vs mark_utf8 vs mark_utf16 indexing");
    let doc = make_doc();
    let text = doc.get_text("body");
    text.insert(0, "héllo").unwrap();
    eprintln!(
        "  text 'héllo': len_unicode={}, len_utf8={}, len_utf16={}",
        text.len_unicode(),
        text.len_utf8(),
        text.len_utf16()
    );

    text.mark(1..2, "bold", true).unwrap();
    doc.commit();
    eprintln!("  after mark(1..2, 'bold') (Unicode scalar offsets):");
    show_delta(&text);
}

fn s_unmark_caveat() {
    header("S11: unmark on a mark applied by another peer (cross-peer unmark)");
    let a = make_doc();
    let b = make_doc();
    a.set_peer_id(1).unwrap();
    b.set_peer_id(2).unwrap();
    let ta = a.get_text("body");
    ta.insert(0, "0123456789").unwrap();
    ta.mark(2..7, "bold", true).unwrap();
    a.commit();
    let snap = a.export(ExportMode::all_updates()).unwrap();
    b.import(&snap).unwrap();

    let tb = b.get_text("body");
    tb.unmark(2..7, "bold").unwrap();
    b.commit();
    let snap_b = b.export(ExportMode::all_updates()).unwrap();
    a.import(&snap_b).unwrap();
    eprintln!("  delta on A after B unmarked the bold A originally applied:");
    show_delta(&a.get_text("body"));
}

fn main() {
    eprintln!("Loro marks spike (loro 1.11.1) — Phase 0.1\n");
    s_basic_mark_unmark_delta();
    s_config_late_text_creation();
    s_reconfigure_conflict();
    s_concurrent_bold_insert();
    s_concurrent_unmark_inside_bold();
    s_concurrent_link_conflict();
    s_link_with_loro_map_value();
    s_cursor_across_remote_insert();
    s_cursor_across_mark_only();
    s_indexing_flavors();
    s_unmark_caveat();
    eprintln!("\n=== END ===");
}
