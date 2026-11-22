//! `parse` and `scan_inline` are total on authored text: `Ok` or `Err`, never
//! a panic, and time linear in the line length.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use holon_api::EntityUri;
use holon_markdown::LogseqMarkdownAdapter;
use holon_markdown::ObsidianMarkdownAdapter;
use holon_markdown::inline::scan_inline;

use crate::documents::Flavor;
use crate::documents::every_doc;

#[test]
fn parse_never_panics_on_enumerated_documents() {
    for flavor in Flavor::ALL {
        for doc in every_doc(flavor, 3) {
            let _ = flavor.parse(&doc.source);
        }
    }
}

/// An inline scan of `text` finishing inside `bound`. The quadratic scan needs
/// hours for these inputs and the linear one milliseconds, so the bound cannot
/// pass or fail by machine load.
fn finishes_within(text: String, bound: Duration) {
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = scan_inline(&text);
        let _ = done.send(());
    });
    finished
        .recv_timeout(bound)
        .unwrap_or_else(|_| panic!("scan_inline did not finish within {bound:?}"));
}

const HANG_BOUND: Duration = Duration::from_secs(10);
const LONG: usize = 1_000_000;

#[test]
fn a_line_of_unclosed_openers_finishes() {
    for opener in ["[", "[[", "((", "*", "`"] {
        finishes_within(opener.repeat(LONG / opener.len()), HANG_BOUND);
    }
}

#[test]
fn a_line_of_openers_with_one_closer_at_the_end_finishes() {
    for (opener, closer) in [("[", "]"), ("[[", "]]"), ("((", "))"), ("*", "*")] {
        finishes_within(
            format!("{}{closer}", opener.repeat(LONG / opener.len())),
            HANG_BOUND,
        );
    }
}

/// A line of `count` pairwise distinct tags, in each tag spelling.
fn distinct_tag_line(count: usize, spelling: fn(usize) -> String) -> String {
    (0..count).map(spelling).collect()
}

#[test]
fn a_line_of_distinct_tags_finishes() {
    let spellings: [fn(usize) -> String; 2] = [|i| format!("#t{i} "), |i| format!("#[[t {i}]] ")];
    for spelling in spellings {
        finishes_within(distinct_tag_line(100_000, spelling), HANG_BOUND);
    }
}

/// Each case is one precondition or one IO state; there is no input space to
/// enumerate.
#[test]
fn a_path_outside_the_vault_root_is_refused_naming_both() {
    let outside = Path::new("/elsewhere/P.md");
    let root = Path::new("/vault");
    for adapter in [
        &LogseqMarkdownAdapter::new() as &dyn holon_core::file_format::FileFormatAdapter,
        &ObsidianMarkdownAdapter::new(),
    ] {
        let message = format!(
            "{:#}",
            adapter
                .parse(outside, "- a\n", &EntityUri::no_parent(), root)
                .err()
                .expect("the parse is refused")
        );
        assert!(message.contains("/elsewhere/P.md"), "{message}");
        assert!(message.contains("/vault"), "{message}");
    }
}

#[cfg(unix)]
#[test]
fn a_file_name_that_is_not_utf8_is_refused_naming_the_path() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let root = Path::new("/vault");
    let path = root.join(OsStr::from_bytes(b"caf\xe9.md"));
    for adapter in [
        &LogseqMarkdownAdapter::new() as &dyn holon_core::file_format::FileFormatAdapter,
        &ObsidianMarkdownAdapter::new(),
    ] {
        let message = format!(
            "{:#}",
            adapter
                .parse(&path, "- a\n", &EntityUri::no_parent(), root)
                .err()
                .expect("the parse is refused")
        );
        assert!(message.contains("not valid UTF-8"), "{message}");
        assert!(message.contains("caf"), "{message}");
    }
}

#[test]
fn an_unprobeable_vault_root_is_an_error_not_an_org_vault() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("not-a-dir");
    std::fs::write(&file, "x").unwrap();
    let err = holon_markdown::detect_flavor(&file).expect_err("the probe fails");
    assert!(format!("{err:#}").contains("not-a-dir"), "{err:#}");
}
