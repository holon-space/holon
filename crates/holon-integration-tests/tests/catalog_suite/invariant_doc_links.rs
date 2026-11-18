//! Every doc clause an invariant names (`Attribution::checks`) must resolve to
//! exactly one `<a id="slug"></a>` anchor in the named file; slugged clauses
//! that no invariant names are reported, not failed.
//!
//! @pbt kind infra
//! @pbt covers invariant-doc-links — a link that points nowhere fails by
//! invariant id and link

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use holon_integration_tests::pbt::composed::catalog::composed_invariant_catalog;

fn repo_root() -> PathBuf {
    Path::new(
        &std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR at run time"),
    )
    .join("../..")
}

fn is_slug(s: &str) -> bool {
    !s.is_empty()
        && s.split('-').all(|seg| {
            !seg.is_empty()
                && seg
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// `docs/<path>.md#<slug>` into (path, slug).
fn parse_link(link: &str) -> Result<(&str, &str), String> {
    let (path, slug) = link
        .split_once('#')
        .ok_or_else(|| "missing `#<slug>`".to_string())?;
    if !(path.starts_with("docs/") && path.ends_with(".md")) {
        return Err("file must be `docs/<path>.md`".to_string());
    }
    if !is_slug(slug) {
        return Err(format!("slug `{slug}` is not [a-z0-9]+(-[a-z0-9]+)*"));
    }
    Ok((path, slug))
}

/// Slugs of every `<a id="slug"></a>` line, with repetition.
fn anchors_of(text: &str) -> Vec<&str> {
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("<a id=\"")?.strip_suffix("\"></a>"))
        .filter(|s| is_slug(s))
        .collect()
}

fn md_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display())) {
        let p = e.unwrap().path();
        if p.is_dir() {
            md_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "md") {
            out.push(p);
        }
    }
}

/// One message per broken link, naming the invariant and the link.
fn broken_links(root: &Path, links: &[(String, Vec<&str>)]) -> Vec<String> {
    let mut failures = Vec::new();
    for (id, clauses) in links {
        for link in clauses {
            let fail = |why: String| failures_push(id, link, why);
            let (path, slug) = match parse_link(link) {
                Ok(v) => v,
                Err(why) => {
                    failures.push(fail(why));
                    continue;
                }
            };
            let text = match std::fs::read_to_string(root.join(path)) {
                Ok(t) => t,
                Err(e) => {
                    failures.push(fail(format!("cannot read {path}: {e}")));
                    continue;
                }
            };
            let n = anchors_of(&text).iter().filter(|a| **a == slug).count();
            if n != 1 {
                failures.push(fail(format!(
                    "{n} anchors with that slug in {path}, expected 1"
                )));
            }
        }
    }
    failures
}

fn failures_push(id: &str, link: &str, why: String) -> String {
    format!("invariant `{id}` links `{link}`: {why}")
}

fn duplicate_slugs(root: &Path, files: &BTreeSet<&str>) -> Vec<String> {
    let mut out = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(root.join(f)).unwrap();
        let mut seen = BTreeSet::new();
        for a in anchors_of(&text) {
            if !seen.insert(a) {
                out.push(format!("{f}: duplicate anchor `{a}`"));
            }
        }
    }
    out
}

#[test]
fn every_invariant_doc_link_resolves() {
    let root = repo_root();
    let catalog = composed_invariant_catalog();
    let links: Vec<(String, Vec<&str>)> = catalog
        .iter()
        .map(|i| {
            (
                i.id().0.to_string(),
                i.attribution().checked_clauses().to_vec(),
            )
        })
        .collect();

    let mut failures = broken_links(&root, &links);
    let linked_files: BTreeSet<&str> = links
        .iter()
        .flat_map(|(_, cs)| cs.iter().filter_map(|c| parse_link(c).ok().map(|(p, _)| p)))
        .collect();
    failures.extend(duplicate_slugs(&root, &linked_files));
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));

    let named: BTreeSet<String> = links
        .iter()
        .flat_map(|(_, cs)| cs.iter().map(|c| c.to_string()))
        .collect();
    let mut files = Vec::new();
    md_files(&root.join("docs"), &mut files);
    let mut unnamed = BTreeMap::new();
    for f in files {
        let rel = f
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        for a in anchors_of(&std::fs::read_to_string(&f).unwrap()) {
            if !named.contains(&format!("{rel}#{a}")) {
                unnamed.insert(format!("{rel}#{a}"), ());
            }
        }
    }
    let unlinked_invariants = links.iter().filter(|(_, cs)| cs.is_empty()).count();
    let report = format!(
        "invariants: {}\nwithout a link: {unlinked_invariants}\nslugged clauses no invariant names: {}\n{}\n",
        links.len(),
        unnamed.len(),
        unnamed.keys().cloned().collect::<Vec<_>>().join("\n"),
    );
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("invariant-doc-links-report.txt");
    std::fs::write(&out, &report).unwrap();
    println!("{report}(written to {})", out.display());
}

#[test]
fn a_link_to_a_missing_slug_fails_naming_invariant_and_link() {
    let planted = vec![(
        "inv-planted".to_string(),
        vec!["docs/Architecture/RenderPipeline.md#no-such-slug"],
    )];
    let failures = broken_links(&repo_root(), &planted);
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].contains("inv-planted") && failures[0].contains("#no-such-slug"));
}

mod grammar {
    use proptest::prelude::*;

    use super::*;

    fn slug() -> impl Strategy<Value = String> {
        prop::collection::vec("[a-z0-9]{1,6}", 1..4).prop_map(|s| s.join("-"))
    }

    fn near_miss() -> impl Strategy<Value = String> {
        prop_oneof![
            slug()
                .prop_map(|s| s.to_uppercase())
                .prop_filter("has a letter", |s| s
                    .chars()
                    .any(|c| c.is_ascii_uppercase())),
            slug().prop_map(|s| format!("{s}-")),
            slug().prop_map(|s| format!("-{s}")),
            slug()
                .prop_map(|s| s.replacen('-', "--", 1))
                .prop_filter("has a dash", |s| s.contains("--")),
            slug().prop_map(|s| format!("{s}_x")),
            Just(String::new()),
        ]
    }

    proptest! {
        #[test]
        fn links_parse_iff_slug_in_grammar(good in slug(), bad in near_miss()) {
            let ok = format!("docs/Architecture/X.md#{good}");
            prop_assert_eq!(parse_link(&ok), Ok(("docs/Architecture/X.md", good.as_str())));
            let bad_link = format!("docs/Architecture/X.md#{bad}");
            prop_assert!(parse_link(&bad_link).is_err(), "{}", bad_link);
            let no_hash = format!("docs/Architecture/X.md{}", good);
            prop_assert!(parse_link(&no_hash).is_err());
            let no_docs = format!("Architecture/X.md#{}", good);
            prop_assert!(parse_link(&no_docs).is_err());
        }

        #[test]
        fn anchor_lines_yield_exactly_grammar_slugs(good in slug(), bad in near_miss()) {
            let text = format!("<a id=\"{good}\"></a>\n# H\n<a id=\"{bad}\"></a>\n");
            prop_assert_eq!(anchors_of(&text), vec![good.as_str()]);
        }
    }
}
