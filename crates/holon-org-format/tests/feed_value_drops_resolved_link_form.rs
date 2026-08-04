//! The org bytes a block renders to depend on whether its `page` link mark was
//! upgraded from `EntityRef::Name` to `EntityRef::Scheme`.
//!
//! `CacheBlockReader::get_blocks` / `get_block_authoritative` run
//! `substitute_resolved_links` — a documented RENDER-TIME upgrade that leaves
//! the stored marks untouched. The `block` matview projects `b.marks`
//! verbatim, so a `Block` taken from the block FEED never carries the upgrade.
//! Any write-back that renders feed-sourced values therefore emits the
//! pre-upgrade form, silently reverting `[[<id>][<label>]]` to `[[<label>]]`.

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
