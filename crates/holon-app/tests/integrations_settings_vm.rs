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
use holon_app::integrations_settings::IntegrationsSettingsVm;
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
    (IntegrationsSettingsVm::new(store.clone()), store)
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
    holon_app::McpIntegrationsModule::from_dir(dir.path())
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
    let err = holon_app::McpIntegrationsModule::from_dir(dir.path())
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
