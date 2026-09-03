//! Inc 2's kill criterion: a full-vault scan through the interpreter, against
//! the 200 ms interaction→projection-visible SLO.
//!
//! The measurement is release-only — a debug wasmi is an order of magnitude
//! slower than the one that ships — so the scan is `#[ignore]`d and run
//! explicitly with `--release`. What the default suite still gates is the fuel
//! and memory HEADROOM, which is instruction-counted and therefore identical
//! in both profiles.

use std::path::PathBuf;
use std::time::Instant;

use holon_api::EntityUri;
use holon_core::file_format::FileFormatAdapter;
use holon_plugin_host::PluginHost;
use holon_plugin_host::PluginLimits;

mod support;

/// A 200-step recipe is far larger than any real one; the margin between what
/// it spends and [`PluginLimits::default`] is what keeps a legitimate file from
/// ever hitting a limit meant for a runaway guest.
#[test]
fn fuel_and_memory_headroom() {
    let wasm = std::fs::read(support::plugins_dir().join("cooklang.wasm")).unwrap();
    let limits = PluginLimits::default();
    let mut host = PluginHost::from_bytes(&wasm, limits).unwrap();

    let recipe = support::big_recipe(200);
    let ctx = br#"{"source_path":"Rezepte/Large.cook","file_stem":"Large"}"#;
    host.parse(recipe.as_bytes(), ctx)
        .expect("a 200-step recipe is an ordinary parse");

    let spent = limits.fuel_per_call - host.fuel_remaining();
    let memory = host.memory_bytes();
    println!("200-step recipe: {spent} fuel, {memory} bytes of guest memory");

    assert!(
        spent * 4 < limits.fuel_per_call,
        "the fuel budget must leave a 4x margin over the largest legitimate file: {spent} of {}",
        limits.fuel_per_call
    );
    assert!(
        memory * 4 < limits.memory_bytes,
        "the memory budget must leave a 4x margin: {memory} of {}",
        limits.memory_bytes
    );
}

/// 200 generated recipes through ONE host — the vault scan. Reports against
/// the 200 ms SLO rather than asserting it, because the SLO covers the whole
/// interaction and this measures only the parse leg of it.
#[test]
#[ignore = "release-only measurement; run with --release --run-ignored all"]
fn two_hundred_recipes_on_one_host() {
    let plugin = support::cook_plugin();
    let vault = support::generated_vault(200);
    let root = PathBuf::from("/vault");

    // One warm pass, so the figure is steady-state rather than first-touch.
    for (rel, content) in vault.iter().take(5) {
        plugin
            .parse(&root.join(rel), content, &EntityUri::no_parent(), &root)
            .unwrap();
    }

    let started = Instant::now();
    let mut rows = 0usize;
    for (rel, content) in &vault {
        let parsed = plugin
            .parse(&root.join(rel), content, &EntityUri::no_parent(), &root)
            .expect("every generated recipe parses");
        rows += parsed
            .typed_rows
            .iter()
            .map(|s| s.rows.len())
            .sum::<usize>();
    }
    let elapsed = started.elapsed();
    let bytes: usize = vault.iter().map(|(_, c)| c.len()).sum();

    println!(
        "VAULT SCAN: {} recipes, {bytes} bytes, {rows} rows in {:.1} ms ({:.3} ms per recipe) — \
         SLO is 200 ms per interaction",
        vault.len(),
        elapsed.as_secs_f64() * 1000.0,
        elapsed.as_secs_f64() * 1000.0 / vault.len() as f64
    );
}

/// The same 200 recipes through the native adapter, so the interpreter's cost
/// is a ratio rather than an absolute nobody can place.
#[test]
#[ignore = "release-only measurement; run with --release --run-ignored all"]
fn two_hundred_recipes_natively() {
    let native = holon_kitchen::CookFormatAdapter::new();
    let vault = support::generated_vault(200);
    let root = PathBuf::from("/vault");

    let started = Instant::now();
    for (rel, content) in &vault {
        native
            .parse(&root.join(rel), content, &EntityUri::no_parent(), &root)
            .unwrap();
    }
    let elapsed = started.elapsed();
    println!(
        "NATIVE SCAN: {} recipes in {:.1} ms ({:.3} ms per recipe)",
        vault.len(),
        elapsed.as_secs_f64() * 1000.0,
        elapsed.as_secs_f64() * 1000.0 / vault.len() as f64
    );
}
