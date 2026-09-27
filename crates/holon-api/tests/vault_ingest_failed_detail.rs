//! A file the ingest refuses is disclosed by path and by reason, both whole:
//! the render caps a detail's headline, and these are what the user acts on.

use holon_api::ConditionKind;

#[test]
fn a_refused_file_carries_its_whole_path_and_reason_outside_the_headline() {
    let path = "/Users/someone/vault/Projects/A folder with a long name/Another one/Notes.org";
    let reason = format!(
        "org heading id in file:Notes.org is not usable: {}the id \"a#b\" cannot be written",
        "context of the refusal, ".repeat(20)
    );
    let detail = ConditionKind::VaultIngestFailed {
        format: "org".to_string(),
        reason: reason.clone(),
    }
    .detail(path);
    assert!(
        detail.headline.contains("Notes.org"),
        "the headline names the file: {detail:?}"
    );
    assert!(
        detail.body.iter().any(|line| line == path),
        "the whole path is a body line: {detail:?}"
    );
    assert!(
        detail.body.contains(&reason),
        "the whole reason is a body line: {detail:?}"
    );
}
