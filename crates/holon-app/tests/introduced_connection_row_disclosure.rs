//! An introduced connection's row says where it came from and who it talks to.
//!
//! The threat model for user-installed connections rests on one sentence:
//! enabling one is a DELIBERATE act with a disclosure. A row that shows only a
//! name and an icon does not carry that — both are chosen by the file, so a
//! hostile connection can call itself "Calendar", wear a calendar glyph, and
//! ask for the same click as a bundled one.
//!
//! What the file cannot fake is where it sits on disk and which hosts its
//! manual actually calls. Those two are the disclosure, and they belong on the
//! row the user clicks rather than in a boot log nobody reads at the moment of
//! deciding.
//!
//! Bundled connections carry neither: their origin is the build, and saying
//! "shipped with Holon" on six rows would make the words on the seventh read
//! as decoration rather than as the warning they are.

use std::path::Path;

use holon_app::integrations_settings::IntegrationsSettingsVm;
use holon_mcp_client::IntegrationConfigStore;

fn sidecar_calling(hosts: &[&str]) -> String {
    let tools: String = hosts
        .iter()
        .enumerate()
        .map(|(i, h)| {
            format!(
                "    - name: list{i}\n      tool_call_template:\n        call_template_type: \
                 http\n        http_method: GET\n        url: https://{h}/things\n"
            )
        })
        .collect();
    let holon_tools: String = (0..hosts.len())
        .map(|i| format!("    list{i}: {{}}\n"))
        .collect();
    format!(
        "schema_version: {}\ndisplay_name: \"Calendar\"\nutcp:\n  utcp_version: \"1.1.3\"\n  \
         manual_version: \"1.0.0\"\n  tools:\n{tools}holon:\n  tools:\n{holon_tools}entities: \
         {{}}\ntools: {{}}\n",
        holon_mcp_client::SIDECAR_SCHEMA_VERSION
    )
}

fn vm_over(dir: &Path) -> IntegrationsSettingsVm {
    IntegrationsSettingsVm::new(
        std::sync::Arc::new(IntegrationConfigStore::load(dir).expect("store loads")),
        holon_mcp_client::CredentialRoot::new(dir),
    )
}

#[test]
fn an_introduced_rows_origin_is_the_file_it_came_from() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("calendar.yaml");
    std::fs::write(&path, sidecar_calling(&["api.evil.example"])).unwrap();

    let vm = vm_over(dir.path());
    let row = vm
        .rows()
        .into_iter()
        .find(|r| r.provider == "calendar")
        .expect("the introduced connection has a row");

    let origin = row
        .origin
        .as_ref()
        .expect("an introduced connection must disclose where it came from");
    assert!(
        origin.contains("calendar.yaml"),
        "the origin must name the FILE, which is the one thing the file's author cannot rename \
         away from the user's own directory listing; got {origin:?}"
    );
}

#[test]
fn an_introduced_rows_hosts_are_the_ones_its_manual_calls() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("calendar.yaml"),
        sidecar_calling(&["api.evil.example", "cdn.evil.example"]),
    )
    .unwrap();

    let vm = vm_over(dir.path());
    let row = vm
        .rows()
        .into_iter()
        .find(|r| r.provider == "calendar")
        .expect("the introduced connection has a row");

    assert_eq!(
        row.hosts,
        vec![
            "api.evil.example".to_string(),
            "cdn.evil.example".to_string()
        ],
        "every host the manual calls must be disclosed, de-duplicated and ordered, so the user \
         reads the same list whatever order the tools are authored in"
    );
}

/// The display name is the file's choice, so it is exactly what the disclosure
/// must not rely on. This states that the row keeps BOTH.
#[test]
fn the_disclosure_sits_beside_the_name_the_file_chose() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("calendar.yaml"),
        sidecar_calling(&["api.evil.example"]),
    )
    .unwrap();

    let vm = vm_over(dir.path());
    let row = vm
        .rows()
        .into_iter()
        .find(|r| r.provider == "calendar")
        .expect("row");

    assert_eq!(
        row.display_name, "Calendar",
        "the file's chosen name is still shown — the disclosure sits beside it, not instead of it"
    );
    assert!(row.origin.is_some() && !row.hosts.is_empty());
}

#[test]
fn a_bundled_row_discloses_no_origin_and_no_hosts() {
    let dir = tempfile::tempdir().unwrap();
    let vm = vm_over(dir.path());
    let row = vm
        .rows()
        .into_iter()
        .find(|r| r.provider == "todoist")
        .expect("todoist is bundled");

    assert!(
        row.origin.is_none(),
        "a bundled connection's origin is the build; saying so on every row would make the words \
         on an introduced one read as decoration"
    );
    assert!(row.hosts.is_empty(), "same reason: {:?}", row.hosts);
}

/// A file that OVERRIDES a bundled connection is not an introduction, so it
/// gets no origin line — the connection is still the one the build ships.
#[test]
fn an_overriding_file_for_a_bundled_name_discloses_nothing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("todoist.yaml"),
        sidecar_calling(&["api.evil.example"]),
    )
    .unwrap();

    let vm = vm_over(dir.path());
    let row = vm
        .rows()
        .into_iter()
        .find(|r| r.provider == "todoist")
        .expect("todoist is bundled");
    assert!(
        row.origin.is_none(),
        "an override changes a shipped connection's content, which the superseded/override \
         disclosure already reports; it does not introduce one"
    );
}

/// `rows()` runs on EVERY render of the settings modal, so a warning emitted
/// unconditionally for an unreadable sidecar is written as fast as the UI
/// redraws. The loader's `Unusable` disclosure and its toast are what tell the
/// user; the log line only helps whoever reads the log, and it helps exactly as
/// much when written once.
///
/// Asserted as "the rows are stable and cheap to re-ask for", which is the
/// observable half: a second call must produce the same rows without the list
/// degrading, and the warn-once set makes the log side true by construction.
#[test]
fn repeated_rows_calls_are_stable_for_an_unreadable_introduced_file() {
    let dir = tempfile::tempdir().unwrap();
    // Names a connection, but declares no schema_version — Martin's real
    // `github.yaml` shape.
    std::fs::write(
        dir.path().join("github.yaml"),
        "transport:\n  http:\n    uri: https://api.github.com\nentities: {}\n",
    )
    .unwrap();

    let vm = vm_over(dir.path());
    let first = vm.rows();
    let second = vm.rows();
    assert_eq!(
        first, second,
        "the settings list must be the same on every render, whatever the file does"
    );
    let row = first
        .iter()
        .find(|r| r.provider == "github")
        .expect("an unreadable introduced file still gets a row the user can act on");
    assert!(
        row.origin.is_some(),
        "even unreadable, the row must say WHERE the file is — that is what the user has to go \
         and fix"
    );
    assert!(
        row.hosts.is_empty(),
        "and it must claim no hosts, because nothing could read them: {:?}",
        row.hosts
    );
}
