//! ENABLEMENT is the store's decision and nobody else's.
//!
//! Presence is settled at compile time (`bundled_sidecars`), so a `*.yaml` in
//! the integrations directory can neither introduce a provider nor switch one
//! on. An integration runs iff it is BUNDLED and its state file says
//! `enabled = true`. Any installed file that consequently does nothing is
//! reported, because a file the user put there deliberately, silently ignored,
//! is the exact failure this crate exists to refuse.

use std::path::Path;

use holon_mcp_client::IgnoredReason;
use holon_mcp_client::IntegrationConfigStore;
use holon_mcp_client::LoadedIntegrations;
use holon_mcp_client::integration_state::Configuration;
use holon_mcp_client::integration_state::IntegrationState;

/// Load through a store rooted at the same directory — the production pairing.
fn load(dir: &Path) -> anyhow::Result<LoadedIntegrations> {
    let store = IntegrationConfigStore::load(dir)?;
    holon_mcp_client::load_integration_configs(dir, &store)
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

/// THE CUTOVER RED. No YAML anywhere, yet the provider runs: enablement comes
/// from the store, so nothing on disk needs to be copied to turn one on.
#[test]
fn a_bundled_provider_enabled_in_the_store_loads_without_any_installed_file() {
    let dir = tempfile::tempdir().unwrap();
    enable(dir.path(), "gcal");

    let loaded = load(dir.path()).expect("load");
    assert!(
        loaded.configs.iter().any(|(n, _)| n == "gcal"),
        "enabled + bundled must load with no YAML on disk — got {:?}",
        loaded.configs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
    assert!(loaded.ignored.is_empty());
}

/// The other half of the cutover: the file that used to be the switch is not.
#[test]
fn an_installed_file_no_longer_enables_a_bundled_provider() {
    let dir = tempfile::tempdir().unwrap();
    let bundled = holon_mcp_client::bundled_sidecar("gcal").expect("bundled");
    let installed = dir.path().join("gcal.yaml");
    std::fs::write(&installed, bundled.yaml).unwrap();

    let loaded = load(dir.path()).expect("load");
    assert!(
        loaded.configs.is_empty(),
        "file presence must not enable anything — got {:?}",
        loaded.configs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );

    let ignored = loaded
        .ignored
        .iter()
        .find(|i| i.provider == "gcal")
        .expect("the file that no longer enables anything is disclosed");
    assert_eq!(ignored.installed_path, installed);
    match &ignored.reason {
        IgnoredReason::NotEnabled {
            state_path,
            enabling_state_file,
            ..
        } => {
            assert_eq!(
                state_path,
                &dir.path().join("gcal.state.toml"),
                "the disclosure names the exact file to write"
            );
            assert!(
                enabling_state_file.contains("enabled = true"),
                "and the exact content to put in it: {enabling_state_file}"
            );
        }
        other => panic!("expected NotEnabled, got {other:?}"),
    }
}

/// R2-B. The disclosure names a state file AND a command; running that command
/// verbatim must produce THAT file. Under an override directory — which is what
/// every sandbox and dogfood run uses — a remedy that writes to the default
/// location would leave the provider still not loading, having reported
/// success.
#[test]
fn the_disclosed_remedy_writes_the_file_it_names_under_an_override_dir() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("todoist.yaml"), "entities: {}\n").unwrap();

    let loaded = load(dir.path()).expect("load");
    let IgnoredReason::NotEnabled {
        state_path, remedy, ..
    } = &loaded
        .ignored
        .iter()
        .find(|i| i.provider == "todoist")
        .expect("disclosed")
        .reason
    else {
        panic!("expected NotEnabled");
    };

    // Verbatim: the string the user is shown, run as a shell command.
    let out = std::process::Command::new("bash")
        .arg("-c")
        .arg(remedy)
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .env("HOME", dir.path())
        .output()
        .expect("run the disclosed remedy");
    assert!(
        out.status.success(),
        "the disclosed remedy failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        state_path.exists(),
        "the remedy must write the very file the same disclosure names: {}",
        state_path.display()
    );
    assert!(
        load(dir.path())
            .expect("load")
            .configs
            .iter()
            .any(|(n, _)| n == "todoist"),
        "and that must be what makes the provider load"
    );
}

/// The remedy the disclosure quotes must be the remedy that works — if the
/// snippet drifted from what the store accepts, the disclosure would be a lie.
#[test]
fn the_disclosed_enabling_state_file_actually_enables() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("todoist.yaml"), "entities: {}\n").unwrap();

    let loaded = load(dir.path()).expect("load");
    let IgnoredReason::NotEnabled {
        state_path,
        enabling_state_file,
        ..
    } = &loaded
        .ignored
        .iter()
        .find(|i| i.provider == "todoist")
        .expect("disclosed")
        .reason
    else {
        panic!("expected NotEnabled");
    };
    std::fs::write(state_path, enabling_state_file).unwrap();

    let loaded = load(dir.path()).expect("load");
    assert!(loaded.configs.iter().any(|(n, _)| n == "todoist"));
    assert!(loaded.ignored.is_empty(), "the remedy also ends the notice");
}

/// A state file that exists but says OFF is not a half-measure: no provider,
/// and no notice either when there is no installed file to explain away.
#[test]
fn an_explicitly_disabled_provider_does_not_load() {
    let dir = tempfile::tempdir().unwrap();
    IntegrationConfigStore::load(dir.path())
        .unwrap()
        .set("gmail", IntegrationState::default())
        .unwrap();

    let loaded = load(dir.path()).expect("load");
    assert!(loaded.configs.is_empty());
    assert!(loaded.ignored.is_empty());
}

/// Presence is compile-time, so a YAML for a name this build does not ship
/// cannot introduce a provider — and must say so rather than vanish.
#[test]
fn an_unbundled_installed_sidecar_is_ignored_and_disclosed() {
    let dir = tempfile::tempdir().unwrap();
    let installed = dir.path().join("my-own-thing.yaml");
    std::fs::write(
        &installed,
        "transport:\n  child_process:\n    command: echo\nentities: {}\n",
    )
    .unwrap();

    let loaded = load(dir.path()).expect("load");
    assert!(loaded.configs.is_empty());
    let ignored = loaded
        .ignored
        .iter()
        .find(|i| i.provider == "my-own-thing")
        .expect("an unbundled file is disclosed, never silently skipped");
    assert_eq!(ignored.installed_path, installed);
    assert!(matches!(ignored.reason, IgnoredReason::NotBundled));
}

/// Enablement moved; the CONTENT axis did not. An enabled provider still runs
/// an installed override that declares this build's sidecar schema version.
#[test]
fn an_enabled_provider_still_honors_a_current_schema_installed_override() {
    let dir = tempfile::tempdir().unwrap();
    enable(dir.path(), "claude-history");
    std::fs::write(
        dir.path().join("claude-history.yaml"),
        format!(
            "schema_version: {}\ntransport:\n  child_process:\n    command: my-own-binary\n\
             entities: {{}}\n",
            holon_mcp_client::SIDECAR_SCHEMA_VERSION
        ),
    )
    .unwrap();

    let loaded = load(dir.path()).expect("load");
    let (_, cfg) = loaded
        .configs
        .iter()
        .find(|(n, _)| n == "claude-history")
        .expect("enabled");
    assert_eq!(
        cfg.transport.child_process.as_ref().unwrap().command,
        "my-own-binary",
        "an override declaring this build's schema version is used as written"
    );
    assert!(loaded.superseded.is_empty());
    assert!(loaded.ignored.is_empty());
}

/// The one-time setup path Martin actually runs must leave a state file this
/// build accepts — a script writing TOML the loader rejects would hand him a
/// working OAuth token and a dead integration.
#[test]
fn the_enable_script_writes_a_state_file_this_build_accepts() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_enable(
        dir.path(),
        &["gcal", "/tmp/id", "/tmp/secret", "/tmp/refresh"],
    );
    assert!(
        out.status.success(),
        "script failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let state = IntegrationConfigStore::load(dir.path())
        .expect("the state file the script wrote must parse")
        .get("gcal")
        .expect("gcal");
    assert!(state.enabled);
    match state.configuration {
        Configuration::Configured(creds) => {
            assert_eq!(creds.refresh_token_file, Path::new("/tmp/refresh"));
        }
        other => panic!("expected Configured, got {other:?}"),
    }

    assert!(
        load(dir.path())
            .expect("load")
            .configs
            .iter()
            .any(|(n, _)| n == "gcal"),
        "and the provider it enabled must load"
    );
}

/// The path to the script every disclosure names, so a test that runs it is
/// running what the user is told to run.
fn enable_script() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(holon_mcp_client::ENABLE_COMMAND)
        .canonicalize()
        .expect("the disclosed command must name a script that exists")
}

fn run_enable(dir: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new("bash")
        .arg(enable_script())
        .args(args)
        .env("HOLON_MCP_INTEGRATIONS_DIR", dir)
        // A test must not be able to write into the real config dir if the
        // script falls back to its default location.
        .env("HOME", dir)
        .output()
        .expect("run the enable script")
}

/// D3. A state file for a name this build does not ship is never read by
/// anything, so writing one must fail rather than report success — the user
/// would otherwise walk away believing an integration is on.
#[test]
fn the_enable_script_refuses_a_provider_this_build_does_not_ship() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_enable(dir.path(), &["gmial"]);
    assert!(
        !out.status.success(),
        "a typo'd provider must fail loud, not print success: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("gmial"), "the error names the typo: {err}");
    assert!(
        err.contains("gmail"),
        "and lists what it could have meant: {err}"
    );
    assert!(
        !dir.path().join("gmial.state.toml").exists(),
        "and writes nothing"
    );
}

/// D3, other half. Even hand-written, an orphan state file must not sit there
/// doing nothing in silence — the loader owns the disclosure.
#[test]
fn an_orphan_state_file_is_disclosed() {
    let dir = tempfile::tempdir().unwrap();
    let orphan = dir.path().join("gmial.state.toml");
    std::fs::write(
        &orphan,
        "schema_version = 1\nenabled = true\n\n[configuration]\nstatus = \"unconfigured\"\n",
    )
    .unwrap();

    let loaded = load(dir.path()).expect("an orphan state file cannot break the load");
    assert!(loaded.configs.is_empty());
    let ignored = loaded
        .ignored
        .iter()
        .find(|i| i.provider == "gmial")
        .expect("a state file for a name this build does not ship is disclosed");
    assert_eq!(ignored.installed_path, orphan);
    assert!(matches!(ignored.reason, IgnoredReason::NotBundled));
}

/// D4. An unbundled file can enable nothing, so it must not be able to abort
/// the boot either — not even two of them.
#[test]
fn duplicate_unbundled_files_are_ignored_not_fatal() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("my-own-thing.yaml"), "entities: {}\n").unwrap();
    std::fs::write(dir.path().join("my-own-thing.yml"), "entities: {}\n").unwrap();

    let loaded = load(dir.path()).expect("two files that enable nothing cannot break the boot");
    assert!(loaded.configs.is_empty());
    assert_eq!(
        loaded.ignored.len(),
        2,
        "each file is named, so neither is silently dropped"
    );
}

/// A BUNDLED name is the opposite case: the two files disagree about what a
/// running integration is, and there is no rule that picks between them.
#[test]
fn duplicate_bundled_files_stay_fatal() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("gcal.yaml"), "entities: {}\n").unwrap();
    std::fs::write(dir.path().join("gcal.yml"), "entities: {}\n").unwrap();

    let err = load(dir.path()).expect_err("two sidecars for one provider has no winner");
    let msg = format!("{err:#}");
    assert!(msg.contains("installed sidecars"), "{msg}");
    assert!(
        msg.contains("gcal.yaml") && msg.contains("gcal.yml"),
        "{msg}"
    );
}

/// D5. One directory, one environment variable — the app's. A script that reads
/// a different name silently writes state the app never looks at.
#[test]
fn the_enable_script_honors_the_app_s_integrations_dir_variable() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_enable(dir.path(), &["todoist"]);
    assert!(
        out.status.success(),
        "script failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        dir.path().join("todoist.state.toml").exists(),
        "HOLON_MCP_INTEGRATIONS_DIR is the variable the app itself reads"
    );
}
