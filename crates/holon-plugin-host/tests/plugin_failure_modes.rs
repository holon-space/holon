//! Every way a plugin can be wrong answers with a NAMED error, never with a
//! silent skip or a half-written file.
//!
//! The guest is `guests/testkit`, one `.wasm` whose behaviour the file's first
//! line selects — so each case below is the real host running real wasm, not a
//! stub standing in for one.

use std::path::Path;
use std::path::PathBuf;

use holon_api::EntityUri;
use holon_core::file_format::FileFormatAdapter;
use holon_core::file_format::WriteTier;
use holon_plugin_host::PluginFormatAdapter;
use holon_plugin_host::PluginHost;
use holon_plugin_host::PluginLimits;

mod support;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn testkit() -> PluginFormatAdapter {
    PluginFormatAdapter::load(&fixtures().join("testkit.yaml"), PluginLimits::default())
        .expect("the testkit sidecar must load")
}

/// Run one behaviour and return whatever the adapter said.
fn run(adapter: &PluginFormatAdapter, behavior: &str) -> anyhow::Result<()> {
    let root = PathBuf::from("/vault");
    adapter
        .parse(
            &root.join("notes/case.testkit"),
            &format!("{behavior}\n"),
            &EntityUri::no_parent(),
            &root,
        )
        .map(|_| ())
}

fn refusal(adapter: &PluginFormatAdapter, behavior: &str) -> String {
    match run(adapter, behavior) {
        Ok(()) => panic!("behaviour {behavior:?} must be refused, and was accepted"),
        Err(e) => format!("{e:#}"),
    }
}

#[test]
fn a_well_formed_guest_is_accepted() {
    let adapter = testkit();
    let root = PathBuf::from("/vault");
    let result = adapter
        .parse(
            &root.join("notes/case.testkit"),
            "well_formed\n",
            &EntityUri::no_parent(),
            &root,
        )
        .expect("the baseline stream is admissible");
    assert_eq!(result.document.content, "notes/case.testkit");
    assert_eq!(result.typed_rows.len(), 1);
    assert_eq!(result.typed_rows[0].rows.len(), 1);
}

/// Bugfunnel `2026-09-02-a-refused-cook-file-still-leaves-a-document-block`:
/// a refused parse must leave NOTHING behind — no scope and no document block.
/// The plugin seam makes that structural rather than a discipline: the whole
/// projection is built from one stream, so a refusal has nothing to return.
#[test]
fn a_refused_file_yields_no_document_and_no_scope() {
    let adapter = testkit();
    for behavior in [
        "refuse",
        "trap",
        "not_a_stream",
        "undeclared_scope",
        "no_document",
    ] {
        let outcome = run(&adapter, behavior);
        assert!(
            outcome.is_err(),
            "behaviour {behavior:?} must produce no partial projection"
        );
    }
}

#[test]
fn a_guest_that_refuses_names_its_reason() {
    let message = refusal(&testkit(), "refuse");
    assert!(
        message.contains("refuses this file by request") && message.contains("testkit"),
        "the refusal must carry the guest's own words and the format name: {message}"
    );
}

#[test]
fn a_guest_that_traps_is_reported_as_a_wasm_error_not_as_empty_rows() {
    let message = refusal(&testkit(), "trap");
    assert!(
        message.contains("wasm") || message.contains("unreachable"),
        "a trap must be named as one: {message}"
    );
}

#[test]
fn a_malformed_envelope_is_refused_by_the_contract() {
    let message = refusal(&testkit(), "not_a_stream");
    assert!(
        message.contains("not the row contract"),
        "the host must say the stream is not the contract: {message}"
    );
}

#[test]
fn an_undeclared_scope_is_refused() {
    let message = refusal(&testkit(), "undeclared_scope");
    assert!(
        message.contains("mystery") && message.contains("does not declare"),
        "{message}"
    );
}

#[test]
fn an_undeclared_column_is_refused() {
    let message = refusal(&testkit(), "undeclared_column");
    assert!(
        message.contains("surprise") && message.contains("does not declare"),
        "{message}"
    );
}

#[test]
fn a_scope_owned_by_the_wrong_column_is_refused() {
    let message = refusal(&testkit(), "wrong_owner_column");
    assert!(message.contains("owner column"), "{message}");
}

/// A row whose owner cell disagrees with its scope would be written outside
/// the scope its own replacement sweeps — it could never be retired.
#[test]
fn a_row_outside_its_own_scope_is_refused() {
    let message = refusal(&testkit(), "row_outside_its_scope");
    assert!(message.contains("outside the scope"), "{message}");
}

/// An id that already reads as a schemed URI is stored unprefixed, leaving
/// every reference to it joining to nothing.
#[test]
fn an_id_that_would_not_land_is_refused() {
    let message = refusal(&testkit(), "unstorable_id");
    assert!(message.contains("already:schemed"), "{message}");
}

#[test]
fn a_declared_scope_the_guest_never_emits_is_refused() {
    let message = refusal(&testkit(), "missing_scope");
    assert!(
        message.contains("emitted no") && message.contains("thing"),
        "a scope left out is how the last row of that type would never get swept: {message}"
    );
}

/// A scope with zero rows is LEGAL and load-bearing: it is how the last row of
/// a set gets swept on re-ingest.
#[test]
fn a_declared_scope_with_no_rows_is_accepted() {
    let adapter = testkit();
    let root = PathBuf::from("/vault");
    let result = adapter
        .parse(
            &root.join("notes/case.testkit"),
            "empty_scope\n",
            &EntityUri::no_parent(),
            &root,
        )
        .expect("an empty scope is how the last row gets swept");
    assert_eq!(result.typed_rows.len(), 1);
    assert!(result.typed_rows[0].rows.is_empty());
}

#[test]
fn two_document_rows_are_refused() {
    let message = refusal(&testkit(), "two_documents");
    assert!(message.contains("exactly one document"), "{message}");
}

/// `partition_params` routes a param naming a storage column straight to that
/// column, so a property named `content` would overwrite the block's own text.
#[test]
fn a_property_naming_a_storage_column_is_refused() {
    let message = refusal(&testkit(), "storage_column_property");
    assert!(
        message.contains("storage column") && message.contains("content"),
        "{message}"
    );
}

#[test]
fn a_guest_that_loops_forever_runs_out_of_fuel() {
    let adapter = PluginFormatAdapter::load(
        &fixtures().join("testkit.yaml"),
        PluginLimits {
            fuel_per_call: 5_000_000,
            ..PluginLimits::default()
        },
    )
    .unwrap();
    let message = refusal(&adapter, "spin");
    assert!(
        message.contains("fuel"),
        "an endless guest must end the call, not the app: {message}"
    );
}

#[test]
fn a_guest_that_allocates_forever_hits_the_memory_limit() {
    let adapter = PluginFormatAdapter::load(
        &fixtures().join("testkit.yaml"),
        PluginLimits {
            memory_bytes: 8 * 1024 * 1024,
            ..PluginLimits::default()
        },
    )
    .unwrap();
    let message = refusal(&adapter, "devour");
    assert!(
        message.contains("memory limit") || message.contains("fuel"),
        "an insatiable guest must be stopped by a limit: {message}"
    );
}

/// A vault where one plugin refuses many files by trapping must not walk the
/// host into its own memory ceiling: the host instance outlives the call, so a
/// buffer left behind by a trapped call is never reclaimed.
///
/// The counter, not the memory size, is the observable: linear memory only
/// grows in 64 KiB pages, so a leak of a few KiB per file hides for thousands
/// of files before it shows.
#[test]
fn a_call_that_traps_leaves_no_buffer_behind_in_the_guest() {
    let mut host = testkit_host(PluginLimits::default());
    let ctx = br#"{"source_path":"notes/case.testkit"}"#;
    let trapping = format!("trap\n{}\n", "x".repeat(20 * 1024));

    let idle = host.guest_live_bytes().unwrap();
    assert_eq!(idle, 0, "a fresh guest lends nothing");

    for i in 0..200 {
        host.parse(trapping.as_bytes(), ctx)
            .expect_err("the testkit guest traps by request");
        assert_eq!(
            host.guest_live_bytes().unwrap(),
            idle,
            "trapped call {i} left its buffers in the guest"
        );
    }

    host.parse(b"ok\n", ctx)
        .expect("a good file still parses after 200 traps");
    assert_eq!(
        host.guest_live_bytes().unwrap(),
        idle,
        "a successful call left its buffers in the guest"
    );
}

/// Fuel exhaustion is the trap that also empties the tank the release itself
/// needs, so it gets its own case rather than riding on the panic path.
#[test]
fn a_call_that_runs_out_of_fuel_leaves_no_buffer_behind_in_the_guest() {
    let mut host = testkit_host(PluginLimits {
        fuel_per_call: 5_000_000,
        ..PluginLimits::default()
    });
    let ctx = br#"{"source_path":"notes/case.testkit"}"#;

    for i in 0..20 {
        host.parse(b"spin\n", ctx)
            .expect_err("an endless guest must run out of fuel");
        assert_eq!(
            host.guest_live_bytes().unwrap(),
            0,
            "fuel-exhausted call {i} left its buffers in the guest"
        );
    }
}

fn testkit_host(limits: PluginLimits) -> PluginHost {
    let wasm =
        std::fs::read(fixtures().join("testkit.wasm")).expect("the testkit guest must exist");
    PluginHost::from_bytes(&wasm, limits).expect("the testkit guest must instantiate")
}

/// A plugin is read-only, and the seam must say so BEFORE any render is
/// attempted rather than by panicking inside the write-back task.
#[test]
fn a_plugin_format_is_read_only_and_refuses_write_back() {
    let adapter = testkit();
    assert_eq!(adapter.write_tier(), WriteTier::ReadOnly);
    let verdict = adapter.writeback_drops(
        Path::new("/vault/notes/case.testkit"),
        "",
        "",
        &[],
        &Default::default(),
        Path::new("/vault"),
    );
    assert!(verdict.is_err(), "a read-only format refuses write-back");
}

#[test]
fn a_sidecar_claiming_write_back_is_refused() {
    let dir = std::env::temp_dir().join("holon-plugin-host-sidecar-tests");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(fixtures().join("testkit.wasm"), dir.join("testkit.wasm")).unwrap();
    let sidecar = dir.join("greedy.yaml");
    std::fs::write(
        &sidecar,
        "format: greedy\nguest: testkit.wasm\nextensions: [greedy]\nwrite_tier: read_write\n\
         scopes:\n  - type: thing\n    owner_column: source_path\n    columns: [id, source_path]\n",
    )
    .unwrap();

    let error = PluginFormatAdapter::load(&sidecar, PluginLimits::default())
        .expect_err("write-back needs a reverse export the contract has no room for")
        .to_string();
    let chain = format!("{:#}", anyhow::anyhow!(error));
    assert!(chain.contains("not admissible"), "{chain}");
}

#[test]
fn a_sidecar_claiming_a_contract_scope_is_refused() {
    let dir = std::env::temp_dir().join("holon-plugin-host-sidecar-tests");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(fixtures().join("testkit.wasm"), dir.join("testkit.wasm")).unwrap();
    let sidecar = dir.join("greedy-scope.yaml");
    std::fs::write(
        &sidecar,
        "format: greedy\nguest: testkit.wasm\nextensions: [greedy2]\nscopes:\n  \
         - type: holon.document\n    owner_column: source_path\n    columns: [id, source_path]\n",
    )
    .unwrap();

    assert!(
        PluginFormatAdapter::load(&sidecar, PluginLimits::default()).is_err(),
        "a sidecar cannot claim the scope that carries blocks"
    );
}

/// The cooklang plugin loads from the sidecar the vault ships, claims `.cook`,
/// and names itself the way a reader of an error would recognise it.
#[test]
fn the_shipped_cooklang_sidecar_registers_the_format() {
    let adapter = support::cook_plugin();
    assert_eq!(adapter.extensions(), &["cook"]);
    assert_eq!(adapter.format_name(), "cooklang");
    assert_eq!(adapter.write_tier(), WriteTier::ReadOnly);
}

/// A format joins the vault by being INSTALLED, not by being wired: the
/// registry builds its adapters from whatever sidecars the directory holds.
#[test]
fn the_plugin_directory_is_the_registration() {
    let installed = PluginFormatAdapter::load_dir(&support::plugins_dir(), PluginLimits::default())
        .expect("the shipped plugin directory must load");
    let names: Vec<&str> = installed.iter().map(|a| a.format_name()).collect();
    assert_eq!(names, vec!["cooklang"]);

    let registry = holon_core::file_format::FormatRegistry::new(
        installed
            .into_iter()
            .map(|a| std::sync::Arc::new(a) as std::sync::Arc<dyn FileFormatAdapter>)
            .collect(),
    )
    .expect("no two installed plugins may claim one extension");
    assert!(registry.handles(Path::new("/vault/Rezepte/Pfannkuchen.cook")));
}

/// A directory with no sidecars is an ordinary vault, not a failure.
#[test]
fn an_absent_plugin_directory_installs_nothing() {
    let missing = std::env::temp_dir().join("holon-plugin-host-no-such-dir");
    assert!(
        PluginFormatAdapter::load_dir(&missing, PluginLimits::default())
            .unwrap()
            .is_empty()
    );
}
