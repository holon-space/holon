//! A file the ingest refuses is disclosed by path and by reason, both whole:
//! the render caps a detail's headline, and these are what the user acts on.
//! Many refused files of one format are one detail that counts them and names
//! the first few.

use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_api::IngestRefusals;
use holon_api::RefusedFile;
use holon_api::condition_detail::ConditionDetail;

fn ingest_detail(bus: &ConditionBus, format: &str) -> ConditionDetail {
    let condition = bus
        .subscribe()
        .current
        .into_iter()
        .find(|c| c.subject == format)
        .expect("the format's condition is raised");
    assert!(matches!(
        condition.reason,
        ConditionKind::VaultIngestFailed(_)
    ));
    condition.reason.detail(&condition.subject)
}

#[test]
fn a_refused_file_carries_its_whole_path_and_reason_outside_the_headline() {
    let path = "/Users/someone/vault/Projects/A folder with a long name/Another one/Notes.org";
    let reason = format!(
        "org heading id in file:Notes.org is not usable: {}the id \"a#b\" cannot be written",
        "context of the refusal, ".repeat(20)
    );
    let bus = ConditionBus::new();
    bus.vault_ingest_refused(
        "org",
        RefusedFile {
            path: path.to_string(),
            reason: reason.clone(),
        },
    );
    let detail = ingest_detail(&bus, "org");
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

#[test]
fn many_refused_files_count_in_the_headline_and_name_the_first_few() {
    let bus = ConditionBus::new();
    let extra = 2;
    let paths: Vec<String> = (0..IngestRefusals::EXAMPLES + extra)
        .map(|i| format!("/vault/recipe-{i}.cook"))
        .collect();
    for path in &paths {
        bus.vault_ingest_refused(
            "cooklang",
            RefusedFile {
                path: path.clone(),
                reason: format!("{path}: unclosed brace"),
            },
        );
    }
    let detail = ingest_detail(&bus, "cooklang");
    assert!(
        detail
            .headline
            .starts_with(&format!("{} cooklang files were not read", paths.len())),
        "the headline counts the files: {detail:?}"
    );
    for path in &paths[..IngestRefusals::EXAMPLES] {
        assert!(detail.body.contains(path), "{path} is named: {detail:?}");
        assert!(
            detail.body.contains(&format!("{path}: unclosed brace")),
            "{path}'s reason is shown: {detail:?}"
        );
    }
    for path in &paths[IngestRefusals::EXAMPLES..] {
        assert!(
            !detail.body.contains(path),
            "{path} is past the examples: {detail:?}"
        );
    }
    assert_eq!(
        detail.body.last().map(String::as_str),
        Some(format!("and {extra} more cooklang files").as_str()),
        "the rest are counted: {detail:?}"
    );
    assert_eq!(
        bus.refused_files("cooklang")
            .into_iter()
            .map(|f| f.path)
            .collect::<Vec<_>>(),
        paths,
        "the full list holds every file"
    );
}
