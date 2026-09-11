//! A file on disk can INTRODUCE a connection — and still cannot switch it on.
//!
//! Presence used to be settled at compile time, so a `<name>.yaml` whose stem
//! matched nothing in the bundle enabled nothing and was merely disclosed.
//! That is what made Martin's hand-written `github.yaml` "run nothing", and it
//! is what ADR 0034 §6 says should not be the whole story: admission is
//! "bundled copies compiled in … plus user overrides from
//! `{config_dir}/connections/*.yaml`".
//!
//! What changes is PRESENCE only. ENABLEMENT stays exactly where it was: a
//! dropped file introduces a POSSIBILITY, never a running connection, and the
//! property `enablement_cutover.rs` pins is extended here rather than relaxed.

use std::path::Path;

use holon_mcp_client::IgnoredReason;
use holon_mcp_client::IntegrationConfigStore;
use holon_mcp_client::LoadedIntegrations;
use holon_mcp_client::integration_state::Configuration;
use holon_mcp_client::integration_state::IntegrationState;

/// A minimal `schema_version: 2` connection. Not a real GitHub manual — this
/// exists to prove presence, enablement and disclosure, not to talk to an API.
fn fixture_yaml() -> String {
    format!(
        r#"
schema_version: {}
display_name: "My Own Thing"
utcp:
  utcp_version: "1.1.3"
  manual_version: "1.0.0"
  tools:
    - name: list
      tool_call_template:
        call_template_type: http
        http_method: GET
        url: https://api.example/things
holon:
  tools:
    list: {{}}
entities: {{}}
tools: {{}}
"#,
        holon_mcp_client::SIDECAR_SCHEMA_VERSION
    )
}

fn load(dir: &Path) -> anyhow::Result<LoadedIntegrations> {
    let store = IntegrationConfigStore::load(dir)?;
    holon_mcp_client::load_integration_configs(
        dir,
        &store,
        &holon_mcp_client::CredentialRoot::new(dir),
    )
}

fn enable(dir: &Path, provider: &str) {
    IntegrationConfigStore::load(dir)
        .expect("store loads")
        .set(
            provider,
            IntegrationState {
                enabled: true,
                configuration: Configuration::Unconfigured,
            },
        )
        .expect("set");
}

fn install(dir: &Path, name: &str, yaml: &str) -> std::path::PathBuf {
    let path = dir.path_join(name);
    std::fs::write(&path, yaml).expect("write the installed sidecar");
    path
}

trait PathJoin {
    fn path_join(&self, name: &str) -> std::path::PathBuf;
}
impl PathJoin for Path {
    fn path_join(&self, name: &str) -> std::path::PathBuf {
        self.join(format!("{name}.yaml"))
    }
}

// ---------------------------------------------------------------------------
// The new capability
// ---------------------------------------------------------------------------

/// THE INC 4 RED. A file the build does not bundle now names a provider the
/// store can switch on, and once switched on it runs.
#[test]
fn an_installed_file_for_an_unbundled_name_introduces_a_connection() {
    let dir = tempfile::tempdir().unwrap();
    install(dir.path(), "my-own-thing", &fixture_yaml());
    enable(dir.path(), "my-own-thing");

    let loaded = load(dir.path()).expect("load");
    assert!(
        loaded.configs.iter().any(|(n, _)| n == "my-own-thing"),
        "an enabled connection introduced by a file must run — got {:?}",
        loaded.configs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}

/// THE PROPERTY THAT MUST NOT BREAK. Dropping the file is not consent. Before
/// this increment this passed vacuously (nothing could be introduced); it must
/// now pass for the real reason.
#[test]
fn an_introduced_connection_does_not_run_until_the_store_says_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let path = install(dir.path(), "my-own-thing", &fixture_yaml());

    let loaded = load(dir.path()).expect("load");
    assert!(
        loaded.configs.is_empty(),
        "a dropped file must not switch itself on — got {:?}",
        loaded.configs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
    let ignored = loaded
        .ignored
        .iter()
        .find(|i| i.provider == "my-own-thing")
        .expect("a file that enabled nothing is always disclosed");
    assert_eq!(ignored.installed_path, path);
    assert!(
        matches!(ignored.reason, IgnoredReason::NotEnabled { .. }),
        "the reason must now be NOT-ENABLED (the remedy is to switch it on), not NOT-BUNDLED — \
         got {:?}",
        ignored.reason
    );
}

/// The store must hold a cell for an introduced name, or the settings toggle
/// has nothing to write and `provider_of` cannot resolve the clicked row.
#[test]
fn the_store_holds_a_cell_for_an_introduced_connection() {
    let dir = tempfile::tempdir().unwrap();
    install(dir.path(), "my-own-thing", &fixture_yaml());

    let store = IntegrationConfigStore::load(dir.path()).expect("store loads");
    assert!(
        store.providers().iter().any(|p| p == "my-own-thing"),
        "the roster must include the introduced name — got {:?}",
        store.providers()
    );
    store
        .get("my-own-thing")
        .expect("an introduced connection has a state cell the toggle can write");
}

/// Bundle first, then introduced connections in file-name order, so adding one
/// never reshuffles the rows above it.
#[test]
fn introduced_connections_append_after_the_bundle_in_file_name_order() {
    let dir = tempfile::tempdir().unwrap();
    install(dir.path(), "zeta-thing", &fixture_yaml());
    install(dir.path(), "alpha-thing", &fixture_yaml());

    let store = IntegrationConfigStore::load(dir.path()).expect("store loads");
    let names: Vec<String> = store
        .providers()
        .iter()
        .map(|p| p.as_str().to_string())
        .collect();
    let bundled: Vec<String> = holon_mcp_client::BUNDLED_SIDECARS
        .iter()
        .map(|s| s.provider.to_string())
        .collect();

    assert_eq!(
        names[..bundled.len()],
        bundled[..],
        "the bundle keeps its order and its position"
    );
    assert_eq!(
        &names[bundled.len()..],
        &["alpha-thing".to_string(), "zeta-thing".to_string()],
        "introduced connections append in file-name order"
    );
}

// ---------------------------------------------------------------------------
// Disclosure — the shape Martin's real github.yaml takes
// ---------------------------------------------------------------------------

/// A file with an unbundled stem and NO `schema_version` is exactly Martin's
/// `github.yaml`. It must get the same loud "does not parse against this
/// build's sidecar format" disclosure the override path already gives — never
/// a silent ignore, and never a bare NOT-BUNDLED that sends the reader looking
/// for a missing feature instead of a stale file.
#[test]
fn an_introduced_file_at_the_wrong_schema_is_disclosed_loudly() {
    let dir = tempfile::tempdir().unwrap();
    let path = install(
        dir.path(),
        "github",
        "transport:\n  http:\n    uri: https://api.github.com\nentities: {}\n",
    );
    enable(dir.path(), "github");

    let loaded = load(dir.path()).expect("a stale file must not take the boot down");
    assert!(
        loaded.configs.iter().all(|(n, _)| n != "github"),
        "a file that does not declare this build's schema_version must not run"
    );
    let disclosure = loaded
        .ignored
        .iter()
        .find(|i| i.provider == "github")
        .expect("a stale introduced file is disclosed");
    assert_eq!(disclosure.installed_path, path);
    let reason = format!("{:?}", disclosure.reason);
    assert!(
        reason.contains("schema_version"),
        "the disclosure must name the schema_version gap, so the reader fixes the FILE rather \
         than hunting a missing feature; got: {reason}"
    );
}

/// Two files for one introduced name is an ambiguity nothing can resolve, and
/// it must be disclosed rather than silently resolved by scan order.
#[test]
fn two_files_for_one_introduced_name_are_disclosed_not_silently_picked() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("dup.yaml"), fixture_yaml()).unwrap();
    std::fs::write(dir.path().join("dup.yml"), fixture_yaml()).unwrap();

    // Two files name no connection at all, so there is nothing to enable — the
    // store has no cell for 'dup'.
    let store = IntegrationConfigStore::load(dir.path()).expect("store loads");
    assert!(
        store.providers().iter().all(|p| p.as_str() != "dup"),
        "an ambiguous name must not enter the roster"
    );

    let loaded = load(dir.path()).expect("an ambiguity is disclosed, not fatal");
    assert!(loaded.configs.iter().all(|(n, _)| n != "dup"));
    let disclosed: Vec<_> = loaded
        .ignored
        .iter()
        .filter(|i| i.installed_path.to_string_lossy().contains("dup."))
        .collect();
    assert_eq!(
        disclosed.len(),
        2,
        "BOTH files must be named — the user has to delete one; got {:?}",
        loaded.ignored
    );
}

/// A file whose stem is not a usable provider name cannot become one, and the
/// refusal happens before the name reaches any path composition.
#[test]
fn a_file_whose_stem_is_not_a_usable_name_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Not A Name.yaml"), fixture_yaml()).unwrap();

    let store = IntegrationConfigStore::load(dir.path()).expect("one bad stem must not be fatal");
    assert!(
        store.providers().iter().all(|p| p.as_str() != "Not A Name"),
        "a stem that is not a usable provider name must not enter the roster"
    );
}

// ---------------------------------------------------------------------------
// A bundled stem still OVERRIDES rather than introducing (D-b, unchanged)
// ---------------------------------------------------------------------------

#[test]
fn a_bundled_stem_still_overrides_and_does_not_double_enter_the_roster() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("todoist.yaml"), fixture_yaml()).unwrap();

    let store = IntegrationConfigStore::load(dir.path()).expect("store loads");
    let todoist_entries = store
        .providers()
        .iter()
        .filter(|p| p.as_str() == "todoist")
        .count();
    assert_eq!(
        todoist_entries, 1,
        "a file for a bundled stem overrides its CONTENT; it must not appear twice in the roster"
    );
}
