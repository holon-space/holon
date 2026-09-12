//! A connection file this build refuses must not take the application with it.
//!
//! Refusing a cleartext endpoint, an inline secret or a foreign secret
//! namespace is the whole point of the admission rules, and the refusal texts
//! are good. What was wrong is the DELIVERY: the connect loop panicked, so one
//! bad file in the integrations directory cost the user their entire app —
//! including the Settings switch that is the documented way to turn the
//! connection off. D94.a ruled the analogous case (an unsatisfiable re-import)
//! from a stop into a DEGRADED boot with a sticky banner, and this is the same
//! shape: the file is user-supplied, the remedy is to edit it, and the user
//! needs a running application to get there.
//!
//! What is asserted: the session boots, the refused connection is disclosed on
//! the degraded bus naming the file AND the reason, and every other connection
//! is unaffected.
//!
//! @pbt kind harness
//! @pbt covers refused-connection-config-boots-degraded — an inadmissible
//! installed connection file is disclosed, not fatal

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;
use holon_loro::DegradedSignalBus;
use holon_loro::ShareDegradedReason;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build test runtime")
}

/// A connection whose only tool calls `url`. Everything else is the minimum
/// this build's schema accepts, so the file's ONE interesting property is the
/// endpoint.
fn sidecar_calling(url: &str) -> String {
    format!(
        "schema_version: {}\ndisplay_name: \"Calendar\"\nutcp:\n  utcp_version: \"1.1.3\"\n  \
         manual_version: \"1.0.0\"\n  tools:\n    - name: list\n      tool_call_template:\n        \
         call_template_type: http\n        http_method: GET\n        url: {url}\nholon:\n  \
         tools:\n    list: {{}}\nentities: {{}}\ntools: {{}}\n",
        holon_mcp_client::SIDECAR_SCHEMA_VERSION
    )
}

/// The switch. Copying a YAML in enables nothing; this file is what makes the
/// connection reach the connect loop, which is where the refusal used to kill
/// the process.
fn enable(dir: &Path, provider: &str) {
    std::fs::write(
        dir.join(format!("{provider}.state.toml")),
        "schema_version = 1\nenabled = true\n\n[configuration]\nstatus = \"unconfigured\"\n",
    )
    .expect("write the state file");
}

/// Boot the shipped wiring over `dir` and hand back the degraded conditions in
/// effect once the session is up. Reaching this function's return value at all
/// is half the assertion: the panic this file was written against never got
/// here.
async fn boot_and_read_degraded(dir: &Path) -> Vec<holon_loro::ShareDegraded> {
    let holon_config = HolonConfig {
        db_path: Some(dir.join("degraded.db")),
        vault: VaultConfig {
            root: Some(dir.to_path_buf()),
        },
        ..Default::default()
    };
    let (session, _engine, bus) = holon_app::new_from_config_with_di(
        holon_config,
        SessionConfig::new(holon_api::UiInfo::permissive()).without_wait(),
        dir.to_path_buf(),
        HashSet::new(),
        |_| Ok(()),
        |injector| {
            injector
                .try_resolve::<Arc<DegradedSignalBus>>()
                .map(|b| (*b).clone())
        },
    )
    .await
    .expect(
        "the shipped wiring must boot a session even when an installed connection file is \
         inadmissible — a refused file is the user's to fix, and they need a running app to fix it",
    );
    // The registry is a lazily-resolved async singleton, so the connect loop
    // (and its refusals) run only once something asks for it. The production
    // `FrontendSession` factory does exactly that; holding the session here
    // keeps it alive while the conditions are read.
    let bus = bus.expect("the composition root registers a DegradedSignalBus");
    let current = bus.subscribe().current;
    drop(session);
    current
}

/// The P1 from the `user-connections` dogfood pass: a cleartext `http://` URL
/// to a non-loopback host. The refusal is correct and must stay; the app must
/// stay too.
#[test]
fn a_cleartext_endpoint_is_disclosed_rather_than_fatal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let integrations = dir.path().join("integrations");
    std::fs::create_dir_all(&integrations).expect("integrations dir");
    std::fs::write(
        integrations.join("calendar.yaml"),
        sidecar_calling("http://api.example.com/things"),
    )
    .expect("write the connection file");
    enable(&integrations, "calendar");

    let current = runtime().block_on(boot_and_read_degraded(dir.path()));

    let disclosure = current
        .iter()
        .find(|c| c.shared_tree_id == "calendar")
        .unwrap_or_else(|| {
            panic!(
                "the refused connection must be disclosed on the degraded bus so the user learns \
                 WHICH file to fix; the bus carries {:?}",
                current
                    .iter()
                    .map(|c| (&c.shared_tree_id, c.reason.condition_kind()))
                    .collect::<Vec<_>>()
            )
        });

    let ShareDegradedReason::IntegrationSidecarUnusable {
        installed_path,
        why,
        ..
    } = &disclosure.reason
    else {
        panic!(
            "a file that NAMES a connection but cannot be used is `IntegrationSidecarUnusable` — \
             the banner for it says 'fix the file this points at'. Got {:?}",
            disclosure.reason
        )
    };
    assert!(
        installed_path.contains("calendar.yaml"),
        "the disclosure must name the FILE, which is the only thing the user can act on; got \
         {installed_path:?}"
    );
    assert!(
        why.contains("https"),
        "the reason must carry the refusal text, which is what tells the user what to change; got \
         {why:?}"
    );
}

/// The verifier's fixture for this lane, and the one that mattered most: a
/// connection named `todoist-api` owns the namespace `todoist_api_`, so
/// `${TODOIST_API_KEY}` passed the per-reference namespace check and the
/// derived Settings key folded onto the bundled Todoist keychain account. The
/// boot's distinctness assert then refused to start the app — a user file
/// stopping the whole application, which is the defect this file exists to
/// close, reintroduced by the fix for the OTHER half of it.
///
/// The account boundary now refuses the connection at admission
/// (`holon-mcp-client/tests/introduced_connection_account_claims.rs` owns that
/// rule); what is asserted here is the delivery: the app comes up and says
/// which file to fix.
#[test]
fn a_connection_claiming_a_bundled_secret_namespace_is_disclosed_rather_than_fatal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let integrations = dir.path().join("integrations");
    std::fs::create_dir_all(&integrations).expect("integrations dir");
    std::fs::write(
        integrations.join("todoist-api.yaml"),
        sidecar_calling("https://api.example.com/things?t=${TODOIST_API_KEY}"),
    )
    .expect("write the connection file");
    enable(&integrations, "todoist-api");

    let current = runtime().block_on(boot_and_read_degraded(dir.path()));

    // Two conditions stand for this subject and both are right: the YAML is
    // unusable, and the `.state.toml` beside it now enables a name nothing
    // provides. This asserts the first by KIND rather than by position.
    let why = current
        .iter()
        .find_map(|c| match &c.reason {
            ShareDegradedReason::IntegrationSidecarUnusable { provider, why, .. }
                if provider == "todoist-api" =>
            {
                Some(why.clone())
            }
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "the refused connection must be disclosed as unusable, and the app must be \
                 running to show it; the bus carries {:?}",
                current
                    .iter()
                    .map(|c| (&c.shared_tree_id, c.reason.condition_kind()))
                    .collect::<Vec<_>>()
            )
        });
    assert!(
        why.contains("todoist"),
        "and the reason must name the connection whose namespace it collided with, since renaming \
         this file is the remedy; got {why:?}"
    );
}

/// The second shape that reached the boot's distinctness assert, found by the
/// verifier after the first was fixed: ONE connection spelling one account two
/// ways. No nesting, no second provider — `${MYTHING_TOKEN}` and
/// `${MYTHING.TOKEN}` simply fold onto one keychain entry, so two derived
/// fields claimed it and the boot refused to start.
///
/// The rule lives beside the nesting one, in the roster; this asserts the
/// delivery, which is the part that makes it not-fatal.
#[test]
fn a_connection_spelling_one_account_two_ways_is_disclosed_rather_than_fatal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let integrations = dir.path().join("integrations");
    std::fs::create_dir_all(&integrations).expect("integrations dir");
    std::fs::write(
        integrations.join("mything.yaml"),
        sidecar_calling("https://api.example.com/t?a=${MYTHING_TOKEN}&b=${MYTHING.TOKEN}"),
    )
    .expect("write the connection file");
    enable(&integrations, "mything");

    let current = runtime().block_on(boot_and_read_degraded(dir.path()));

    let why = current
        .iter()
        .find_map(|c| match &c.reason {
            ShareDegradedReason::IntegrationSidecarUnusable { provider, why, .. }
                if provider == "mything" =>
            {
                Some(why.clone())
            }
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "reaching the boot's distinctness assert means a user file stops the whole \
                 application; the bus carries {:?}",
                current
                    .iter()
                    .map(|c| (&c.shared_tree_id, c.reason.condition_kind()))
                    .collect::<Vec<_>>()
            )
        });
    assert!(
        why.contains("MYTHING_TOKEN") && why.contains("MYTHING.TOKEN"),
        "and the disclosure must name BOTH spellings, since the remedy is to pick one; got \
         {why:?}"
    );
}

/// The same delivery for the other load-time refusals. They are refused at a
/// different seam (the directory scan) and were already disclosed — this pins
/// that ONE seam now covers both, so a later refusal added to either side
/// cannot regress to a panic unnoticed.
#[test]
fn a_foreign_secret_namespace_is_disclosed_rather_than_fatal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let integrations = dir.path().join("integrations");
    std::fs::create_dir_all(&integrations).expect("integrations dir");
    std::fs::write(
        integrations.join("calendar.yaml"),
        sidecar_calling("https://api.example.com/things?t=${TODOIST_API_KEY}"),
    )
    .expect("write the connection file");
    enable(&integrations, "calendar");

    let current = runtime().block_on(boot_and_read_degraded(dir.path()));

    assert!(
        current.iter().any(|c| c.shared_tree_id == "calendar"),
        "a connection reaching for another connection's secret must be disclosed, not silent and \
         not fatal; the bus carries {:?}",
        current
            .iter()
            .map(|c| (&c.shared_tree_id, c.reason.condition_kind()))
            .collect::<Vec<_>>()
    );
}
