//! A link mark's TARGET is byte-visible in org output: `EntityRef::Name`
//! renders `[[<label>]]`, `EntityRef::Scheme` renders `[[<id>][<label>]]`.
//!
//! Write-back renders the stored mark verbatim, so these bytes are decided by
//! what the user authored — never by a resolution the renderer looked up, and
//! never by which read produced the value. Anything that rewrites a target
//! between store and disk shows up here.

use holon_api::EntityRef;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_org_format::render_lossless;

const LABEL: &str = "Linked Page";
const RESOLVED_ID: &str = "block:550e8400-e29b-41d4-a716-446655440000";

fn link_mark(target: EntityRef) -> Vec<MarkSpan> {
    vec![MarkSpan {
        start: 0,
        end: LABEL.len(),
        mark: InlineMark::Link {
            target,
            label: LABEL.to_string(),
        },
    }]
}

#[test]
fn resolved_and_unresolved_link_targets_render_to_different_bytes() {
    let from_reader = render_lossless(
        LABEL,
        &link_mark(EntityRef::Scheme {
            raw: RESOLVED_ID.to_string(),
        }),
    )
    .expect("render the get_blocks-sourced (substituted) mark");

    let from_feed = render_lossless(
        LABEL,
        &link_mark(EntityRef::Name {
            name: LABEL.to_string(),
        }),
    )
    .expect("render the feed-sourced (unsubstituted) mark");

    assert_eq!(from_reader, format!("[[{RESOLVED_ID}][{LABEL}]]"));
    assert_eq!(from_feed, format!("[[{LABEL}]]"));
    assert_ne!(
        from_reader, from_feed,
        "value provenance is byte-visible in org output"
    );
}
