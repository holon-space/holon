//! Phase 0.3 audit: confirm orgize 0.10.0-alpha.10 produces the inline AST
//! shapes the rich-text plan needs.
//!
//! Run: `cargo run -p holon-orgmode --example inline_ast_audit 2>&1 | tee /tmp/orgize-audit.log`

use orgize::rowan::ast::AstNode;
use orgize::Org;
use orgize::SyntaxKind;

fn header(t: &str) {
    eprintln!("\n=== {t} ===");
}

fn dump_kinds(input: &str, label: &str) {
    eprintln!("  input: {input:?}");
    eprintln!("  label: {label}");
    let org = Org::parse(input);
    let doc = org.document();
    walk(doc.syntax(), 0);
}

fn walk(node: &orgize::SyntaxNode, depth: usize) {
    let pad = "  ".repeat(depth + 1);
    let kind = node.kind();
    let text = node.to_string();
    let snippet = if text.len() > 40 {
        format!("{}…", &text[..40])
    } else {
        text.clone()
    };
    eprintln!(
        "{pad}{kind:?} [{:?}..{:?}] {snippet:?}",
        node.text_range().start(),
        node.text_range().end()
    );
    for child in node.children() {
        walk(&child, depth + 1);
    }
}

fn main() {
    eprintln!("orgize 0.10.0-alpha.10 inline AST audit\n");

    header("1. Each markup type by itself");
    dump_kinds("*bold*", "Bold");
    dump_kinds("/italic/", "Italic");
    dump_kinds("_underline_", "Underline");
    dump_kinds("=verbatim=", "Verbatim");
    dump_kinds("~code~", "Code");
    dump_kinds("+strike+", "Strike");
    dump_kinds("a_{sub}", "Subscript");
    dump_kinds("a^{super}", "Superscript");

    header("2. Links (URL and internal)");
    dump_kinds("[[https://example.com][label]]", "External link with label");
    dump_kinds("[[block:abc-123][go here]]", "Internal block link");
    dump_kinds("[[file:notes.org]]", "File link no description");

    header("3. Nesting cases");
    dump_kinds("*bold _and italic_*", "Bold containing Underline");
    dump_kinds("[[https://x][*bold label*]]", "Link with bold label");
    dump_kinds("*one* and /two/", "Two adjacent markups");

    header("4. Headline with inline markup (Phase 1 'headlines are rich' decision)");
    dump_kinds(
        "* TODO *important* thing",
        "Headline + leading TODO + inline bold",
    );
    dump_kinds("* [[block:abc][a link]]", "Headline containing a link");

    header("5. Escape sequences");
    dump_kinds("\\*not bold\\*", "Backslash-escaped asterisks");
    dump_kinds(
        "a*not bold*b",
        "Markup adjacent to letters (should NOT be bold)",
    );

    header("6. List item adjacency (potential underscore confusion)");
    dump_kinds(
        "- item _underscored_ word",
        "List bullet adjacent to underline",
    );

    header("7. Empty / pathological cases");
    dump_kinds("**", "Two asterisks alone");
    dump_kinds("* *bold*", "Headline whose title starts with bold");

    header("--- Coverage summary ---");
    summarize_kinds();
    eprintln!("\n=== END ===");
}

/// For each markup type we plan to support, parse the canonical example,
/// walk the syntax tree, and print the SyntaxKinds we encounter. Builds
/// the table for docs/orgize_inline_audit.md.
fn summarize_kinds() {
    let cases: &[(&str, &str)] = &[
        ("Bold", "*bold*"),
        ("Italic", "/italic/"),
        ("Underline", "_underline_"),
        ("Verbatim", "=verbatim="),
        ("Code", "~code~"),
        ("Strike", "+strike+"),
        ("Subscript", "a_{sub}"),
        ("Superscript", "a^{super}"),
        ("Link/external", "[[https://x][label]]"),
        ("Link/internal", "[[block:abc][go]]"),
    ];

    eprintln!("  | mark | kinds in tree |");
    eprintln!("  |---|---|");
    for (label, sample) in cases {
        let org = Org::parse(*sample);
        let mut kinds = std::collections::BTreeSet::new();
        for descendant in org.document().syntax().descendants() {
            kinds.insert(format!("{:?}", descendant.kind()));
        }
        let kinds_csv: Vec<&str> = kinds.iter().map(|s| s.as_str()).collect();
        eprintln!("  | {label} | {} |", kinds_csv.join(", "));
    }
    let _ = SyntaxKind::BOLD; // referenced for compile-time confirmation
}
