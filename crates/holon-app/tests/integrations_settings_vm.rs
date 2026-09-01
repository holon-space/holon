//! The integrations settings list — the view model behind the desktop app's
//! Settings → Integrations section.
//!
//! End users never run the enabling script, so the list is the only place the
//! enablement axis is reachable. Three things must hold, and none of them are
//! observable from the store's own tests (which know nothing about a list):
//!
//!  1. The list is exactly the PRESENCE axis — every bundled provider, always,
//!     including ones that are off and ones that were never touched. A list
//!     built from "what loaded" would hide precisely the integrations the user
//!     opened Settings to switch on.
//!  2. A toggle persists through the store and flips the store's signal, so a
//!     second reader (another window, the boot loader) sees the same decision.
//!     Enablement and configuration are independent axes, so toggling one must
//!     not clear the other — that would silently discard an OAuth bootstrap.
//!  3. A state file this build cannot read fails the wiring LOUD, naming the
//!     file. Defaulting would render a list of switches that all say "off"
//!     while the file says otherwise.
//!
//! @pbt kind harness
//! @pbt covers integrations-settings-list — the settings surface lists every
//! bundled integration with its enablement and configuration status, toggles
//! persist through the store's signal, and an unreadable state file fails loud
//! @pbt overlaps integration_state_store — kept: that suite pins the store's
//! own contract, this one pins the list projected on top of it

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use holon_app::integrations_settings::ConfigStatus;
use holon_app::integrations_settings::ConfigureProgress;
use holon_app::integrations_settings::IntegrationsSettingsVm;
use holon_mcp_client::CredentialRoot;
use holon_mcp_client::IntegrationConfigStore;
use holon_mcp_client::integration_state::Configuration;
use holon_mcp_client::integration_state::CredentialRef;
use holon_mcp_client::integration_state::Credentials;
use holon_mcp_client::integration_state::IntegrationState;

/// The providers this build ships, in bundle order — spelled out rather than
/// read from `BUNDLED_SIDECARS` so the test states what the user must see.
const BUNDLED: &[&str] = &[
    "claude-history",
    "gcal",
    "gmail",
    "jsonplaceholder",
    "todoist",
];

fn vm_over(dir: &Path) -> (IntegrationsSettingsVm, Arc<IntegrationConfigStore>) {
    let store = Arc::new(IntegrationConfigStore::load(dir).expect("store loads over a clean dir"));
    (
        IntegrationsSettingsVm::new(store.clone(), CredentialRoot::new(dir)),
        store,
    )
}

/// What an OAuth bootstrap leaves behind on the configuration axis.
fn bootstrapped_credentials() -> Credentials {
    Credentials {
        client_id: CredentialRef::Env {
            var: "GCAL_CLIENT_ID".into(),
        },
        client_secret: CredentialRef::Keychain {
            service: "holon".into(),
            account: "gcal".into(),
        },
        refresh_token_file: PathBuf::from("/tmp/holon-test/gcal.refresh"),
    }
}

#[test]
fn the_list_is_every_bundled_provider_off_and_unconfigured_on_a_clean_vault() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (vm, _store) = vm_over(dir.path());

    let rows = vm.rows();
    let providers: Vec<&str> = rows.iter().map(|r| r.provider).collect();
    assert_eq!(
        providers, BUNDLED,
        "the settings list must show the presence axis in full — every bundled \
         provider, in bundle order"
    );
    for row in &rows {
        assert!(
            !row.enabled,
            "'{}' has no state file, so it must read as off",
            row.provider
        );
        assert_eq!(
            row.status,
            ConfigStatus::Unconfigured,
            "'{}' has no credentials yet",
            row.provider
        );
    }
}

#[test]
fn enabling_writes_a_complete_state_file_and_flips_the_signal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (vm, store) = vm_over(dir.path());
    let signal = store.state("todoist").expect("todoist is bundled");
    assert!(!signal.get_cloned().enabled, "starts off");

    vm.set_enabled("todoist", true).expect("enable todoist");

    assert!(
        signal.get_cloned().enabled,
        "the store's signal must carry the decision, or nothing else in the \
         process ever learns about it"
    );
    let path = store.state_path("todoist").expect("state path");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("state file '{}' must exist: {e}", path.display()));
    assert!(
        text.contains("enabled = true"),
        "the file must record the decision, got:\n{text}"
    );
    assert!(
        text.contains("schema_version = 1"),
        "the file must be a complete, current-generation state file, got:\n{text}"
    );

    let reloaded = IntegrationConfigStore::load(dir.path()).expect("reload");
    assert!(
        reloaded.get("todoist").expect("todoist").enabled,
        "the next launch must see the decision"
    );
}

#[test]
fn switching_off_keeps_the_configuration_axis() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (vm, store) = vm_over(dir.path());
    store
        .set(
            "gcal",
            IntegrationState {
                enabled: true,
                configuration: Configuration::Configured(bootstrapped_credentials()),
            },
        )
        .expect("seed a configured, enabled gcal");

    assert_eq!(
        vm.rows()
            .iter()
            .find(|r| r.provider == "gcal")
            .expect("gcal row")
            .status,
        ConfigStatus::Configured,
        "a bootstrapped integration must read as Configured"
    );

    vm.set_enabled("gcal", false).expect("disable gcal");

    let reloaded = IntegrationConfigStore::load(dir.path()).expect("reload");
    let state = reloaded.get("gcal").expect("gcal");
    assert!(!state.enabled, "the switch is off");
    assert_eq!(
        state.configuration,
        Configuration::Configured(bootstrapped_credentials()),
        "switching off must not discard the OAuth bootstrap — the two axes are \
         independent and re-running the bootstrap is a manual, external step"
    );
}

#[test]
fn a_provider_this_build_does_not_ship_is_rejected_not_ignored() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (vm, _store) = vm_over(dir.path());

    let err = vm
        .set_enabled("notion", true)
        .expect_err("'notion' is not bundled");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("notion") && msg.contains("todoist"),
        "the error must name the unknown provider and what IS bundled, got: {msg}"
    );
}

/// The registration must survive the "nothing to run" container.
///
/// With every integration off, `McpIntegrationsModule::configure` has no
/// config and no ignored sidecar and returns early — and that is precisely the
/// container in which the settings list is the user's only way to switch one
/// on. Registering after that return would leave a fresh install with a
/// permanently empty Integrations section.
#[test]
fn the_settings_list_is_registered_even_when_no_integration_runs() {
    use fluxdi::Module;

    let dir = tempfile::tempdir().expect("tempdir");
    let injector = fluxdi::Injector::root();
    holon_app::McpIntegrationsModule::from_dir(dir.path(), dir.path())
        .configure(&injector)
        .expect("an empty integrations directory is not a wiring failure");

    let vm = injector
        .try_resolve::<IntegrationsSettingsVm>()
        .expect("the settings list must be resolvable with zero integrations enabled");
    assert_eq!(
        vm.rows().len(),
        BUNDLED.len(),
        "the list still shows every bundled integration"
    );
}

#[test]
fn an_unreadable_state_file_does_not_load_as_a_default() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("todoist.state.toml"), "enabled = ").expect("write junk");

    let err = IntegrationConfigStore::load(dir.path())
        .err()
        .expect("a truncated state file must not load as a default");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("todoist") && msg.contains("todoist.state.toml"),
        "the failure must name the provider and the file the user has to fix, \
         got: {msg}"
    );
}

/// The store refusing to load is only half the contract — the other half is
/// that the refusal reaches the boot as a module lifecycle failure instead of
/// being caught and turned into an empty container. A swallowed error here
/// would boot an app whose Integrations section shows five switches all reading
/// "off" while the file on disk says otherwise.
#[test]
fn an_unreadable_state_file_fails_the_module_wiring_loud() {
    use fluxdi::Module;

    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("todoist.state.toml"), "enabled = ").expect("write junk");

    let injector = fluxdi::Injector::root();
    let err = holon_app::McpIntegrationsModule::from_dir(dir.path(), dir.path())
        .configure(&injector)
        .err()
        .expect("a corrupt state file must fail the module, not configure an empty container");

    let msg = format!("{err}");
    assert!(
        msg.contains("todoist") && msg.contains("todoist.state.toml"),
        "the lifecycle failure must carry the provider and the path through to \
         the boot, got: {msg}"
    );
    assert!(
        injector.try_resolve::<IntegrationsSettingsVm>().is_err(),
        "a failed configure must leave NO settings list behind — a half-wired \
         container would render switches over a store that never loaded"
    );
}

// ---------------------------------------------------------------------------
// Round 2 — the consent flow's view-model contract.
// ---------------------------------------------------------------------------

/// Install a sandboxed `gcal` sidecar whose credential paths sit inside `dir`,
/// so no test ever reads the developer's `~/.config/holon/*`.
fn install_sandboxed_gcal(dir: &Path) {
    let bundled =
        holon_mcp_client::bundled_sidecars::bundled_sidecar("gcal").expect("gcal is bundled");
    // Installed verbatim: the sidecar names its credentials `${CONFIG_DIR}/…`,
    // so pointing the rig's config dir at a tempdir sandboxes them by
    // construction. Rewriting the paths here is what a rig had to do back when
    // they resolved against `$HOME` — and a rig that forgot read the real
    // user's credentials.
    std::fs::write(dir.join("gcal.yaml"), bundled.yaml).expect("install sidecar");
}

/// Provision the client id/secret the sandboxed sidecar points at, so a flow
/// gets past credential resolution and parks in the loopback wait.
fn provision_sandboxed_client_credentials(dir: &Path) {
    // `dir` IS the rig's config dir, which is where `${CONFIG_DIR}` resolves.
    let creds = dir;
    for (name, value) in [
        ("gcal-client-id", "sandbox-client-id.apps.example.com"),
        ("gcal-client-secret", "sandbox-client-secret"),
    ] {
        let path = creds.join(name);
        std::fs::write(&path, value).expect("write credential");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
}

struct NoBrowser;

impl holon_mcp_client::oauth_bootstrap::BrowserOpener for NoBrowser {
    fn open(&self, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

/// D6. A second consent flow for the same provider, while one is already
/// waiting on the browser, must be refused rather than started.
///
/// Two concurrent flows race one refresh-token file and share one progress
/// cell, so whichever finishes last decides what the user is told — including a
/// late failure overwriting an earlier success.
#[test]
fn a_second_consent_flow_for_the_same_provider_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    install_sandboxed_gcal(dir.path());
    provision_sandboxed_client_credentials(dir.path());
    let vm = Arc::new(IntegrationsSettingsVm::over_dir(dir.path(), dir.path()).expect("vm"));

    // Park a REAL first flow in its loopback wait, through the public API.
    //
    // On its own thread and runtime: this test's own runtime is the one that
    // must stay free to drive the second attempt below.
    let parked = vm.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime for the parked flow");
        let _ =
            rt.block_on(parked.configure("gcal", &NoBrowser, std::time::Duration::from_secs(120)));
    });

    let progress = vm.configure_progress("gcal");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while progress.get_cloned() != ConfigureProgress::AwaitingConsent {
        assert!(
            std::time::Instant::now() < deadline,
            "the first flow never reached AwaitingConsent; it is {:?}",
            progress.get_cloned()
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime for the second attempt");
    let err = rt
        .block_on(vm.configure("gcal", &NoBrowser, std::time::Duration::from_millis(50)))
        .expect_err("a second flow must be refused while one is in flight");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("already"),
        "the refusal must say a setup is already running, got: {msg}"
    );

    // The refusal must not clobber the running flow's own status line.
    assert_eq!(
        progress.get_cloned(),
        ConfigureProgress::AwaitingConsent,
        "a refused second click must leave the FIRST flow's progress intact"
    );
}

/// A different provider is a different flow, and must not be blocked by one
/// already running elsewhere in the list.
#[test]
fn an_in_flight_flow_does_not_block_a_different_provider() {
    let dir = tempfile::tempdir().expect("tempdir");
    install_sandboxed_gcal(dir.path());
    let vm = Arc::new(IntegrationsSettingsVm::over_dir(dir.path(), dir.path()).expect("vm"));

    // `gmail` has no provisioned credentials, so it refuses on its own merits —
    // what matters is that the refusal is about credentials, not about `gcal`.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let err = rt
        .block_on(vm.configure("gmail", &NoBrowser, std::time::Duration::from_millis(50)))
        .expect_err("gmail is unprovisioned in this rig");
    let msg = format!("{err:#}");
    assert!(
        !msg.contains("already"),
        "another provider's flow must not block this one, got: {msg}"
    );
}

/// D7. An installed sidecar this build cannot use is passed over for the
/// bundled copy. The consent path must not drop that fact: a user who edited
/// that file is likely configuring BECAUSE of the edit, and would otherwise see
/// the flow use different endpoints with nothing to explain it.
#[test]
fn a_superseded_installed_sidecar_is_disclosed_to_the_consent_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    // An installed sidecar that declares no schema_version: structurally valid
    // YAML, but not this build's generation.
    std::fs::write(
        dir.path().join("gcal.yaml"),
        "transport:\n  rest:\n    base_url: https://example.invalid\n    calls: {}\n",
    )
    .expect("install a stale sidecar");

    let content = holon_mcp_client::integration_config::provider_content(dir.path(), "gcal")
        .expect("the bundled copy still governs");
    let reason = content.superseded.unwrap_or_else(|| {
        panic!(
            "a passed-over installed sidecar must be disclosed on the consent path — silently \
             using the bundled copy makes the user's edit look ineffective"
        )
    });
    assert!(
        reason.contains("schema_version"),
        "the disclosure must say WHY it was passed over, got: {reason}"
    );
}
