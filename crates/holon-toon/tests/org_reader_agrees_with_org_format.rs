//! `org_reader` is a SECOND org reader living beside the production one. Two
//! readers of one format are only safe while they give the same answers, so
//! this feeds identical snippets to both and compares the priority they read.
//!
//! Scope is deliberately the priority carriers alone: that is where the two
//! readers were measured to diverge (rev 2 verification) — drawer-only
//! priorities were invisible to this reader, and an invalid cookie was kept as
//! title text instead of being refused.

use std::path::Path;

use holon_api::EntityUri;
use holon_org_format::models::OrgBlockExt;
use holon_org_format::parse_org_file;
use holon_toon::org_reader::parse_org;

/// What a reader made of one headline's priority: the letter it settled on, or
/// a refusal. Reduced to this shape so the two readers' very different error
/// types can still be compared for the only thing that matters — did they agree
/// on what the file means.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Letter(char),
    None,
    Refused,
}

fn org_format_verdict(source: &str) -> Verdict {
    match parse_org_file(
        Path::new("/test/file.org"),
        source,
        &EntityUri::no_parent(),
        Path::new("/test"),
    ) {
        Err(_) => Verdict::Refused,
        Ok(parsed) => {
            let headline = parsed
                .blocks
                .iter()
                .find(|b| b.level() == 1)
                .expect("every fixture has one headline");
            match headline.priority() {
                Some(p) => Verdict::Letter(p.letter()),
                None => Verdict::None,
            }
        }
    }
}

fn toon_verdict(source: &str) -> Verdict {
    match parse_org(source) {
        Err(_) => Verdict::Refused,
        Ok(forest) => {
            let block = &forest.roots[0].block;
            match block.priority {
                Some(p) => Verdict::Letter(p.letter()),
                None => Verdict::None,
            }
        }
    }
}

/// The cases the two readers were measured to disagree on, plus the ones they
/// already agreed on as controls. `expected` is the production reader's answer
/// — it is the authority, and this asserts it explicitly so a drift in EITHER
/// reader shows up as a named failure rather than as two wrongs matching.
#[test]
fn both_org_readers_agree_on_every_priority_carrier() {
    let cases: &[(&str, &str, Verdict)] = &[
        (
            "cookie only",
            "* TODO [#A] Ship it\n:PROPERTIES:\n:ID: p0\n:END:\n",
            Verdict::Letter('A'),
        ),
        (
            "drawer only",
            "* TODO Ship it\n:PROPERTIES:\n:ID: p0\n:priority: A\n:END:\n",
            Verdict::Letter('A'),
        ),
        (
            "uppercase drawer only",
            "* TODO Ship it\n:PROPERTIES:\n:ID: p0\n:PRIORITY: A\n:END:\n",
            Verdict::Letter('A'),
        ),
        (
            "both carriers agreeing",
            "* TODO [#A] Ship it\n:PROPERTIES:\n:ID: p0\n:priority: A\n:END:\n",
            Verdict::Letter('A'),
        ),
        (
            "letter beyond the default range",
            "* TODO [#D] Ship it\n:PROPERTIES:\n:ID: p0\n:END:\n",
            Verdict::Letter('D'),
        ),
        (
            "no carrier at all",
            "* TODO Ship it\n:PROPERTIES:\n:ID: p0\n:END:\n",
            Verdict::None,
        ),
        (
            "cookie and drawer disagree",
            "* TODO [#A] Ship it\n:PROPERTIES:\n:ID: p0\n:priority: B\n:END:\n",
            Verdict::Refused,
        ),
        (
            "two drawer spellings disagree",
            "* TODO Ship it\n:PROPERTIES:\n:ID: p0\n:priority: A\n:PRIORITY: B\n:END:\n",
            Verdict::Refused,
        ),
        (
            "non-ascii cookie letter",
            "* TODO [#Ä] Ship it\n:PROPERTIES:\n:ID: p0\n:END:\n",
            Verdict::Refused,
        ),
        (
            "non-letter drawer value",
            "* TODO Ship it\n:PROPERTIES:\n:ID: p0\n:priority: 1\n:END:\n",
            Verdict::Refused,
        ),
    ];

    for (name, source, expected) in cases {
        assert_eq!(
            &org_format_verdict(source),
            expected,
            "[{name}] control: the production reader must give the authoritative answer"
        );
        assert_eq!(
            &toon_verdict(source),
            expected,
            "[{name}] the second org reader must read the file to mean the same thing"
        );
    }
}
