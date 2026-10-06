//! Inc 2's kill criterion: a full-vault scan through the interpreter, against
//! the 200 ms interaction→projection-visible SLO.
//!
//! The measurement is release-only — a debug wasmi is an order of magnitude
//! slower than the one that ships — so the scan is `#[ignore]`d and run
//! explicitly with `--release`. What the default suite still gates is the fuel
//! and memory HEADROOM, which is instruction-counted and therefore identical
//! in both profiles.

use std::path::PathBuf;
use std::time::Duration;
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
///
/// There is no native leg to divide by any more: the Rust cooklang parser this
/// used to be a ratio against is deleted, so the figure stands against the SLO
/// alone.
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

/// The boot scan parses every `.cook` file in the vault on one thread, and a
/// file is refused only after its parse. Holon is dogfooded on debug builds,
/// so this budget holds in the profile the tests run in, not only in release.
/// It is CPU time on the parsing thread, so machine load does not move it.
const PARSE_BUDGET_PER_RECIPE: Duration = Duration::from_millis(20);

/// Measured cost is 8-10 ms. A reading far below that means the guest ran on
/// another thread, so the budget above would pass without bounding the scan.
const PARSE_FLOOR_PER_RECIPE: Duration = Duration::from_millis(2);

fn thread_cpu_time() -> Duration {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut now) };
    assert_eq!(
        rc,
        0,
        "clock_gettime(CLOCK_THREAD_CPUTIME_ID) failed: {}",
        std::io::Error::last_os_error()
    );
    Duration::new(now.tv_sec as u64, now.tv_nsec as u32)
}

#[test]
fn refusing_a_recipe_stays_within_the_scan_budget_in_every_build_profile() {
    let plugin = support::bundled_cook_plugin();
    let root = PathBuf::from("/vault");
    let recipe = support::big_recipe(20);
    let files = 20;

    let started = Instant::now();
    let cpu_started = thread_cpu_time();
    for i in 0..files {
        let path = root.join(format!("Rezepte/Aargauer Rüeblitorte {i}.cook"));
        let Err(refusal) = plugin.parse(&path, &recipe, &EntityUri::no_parent(), &root) else {
            panic!("a recipe whose path holds a space derives an unstorable id");
        };
        assert!(
            format!("{refusal:#}").contains("is not a storable URI path"),
            "refused for another reason: {refusal:#}"
        );
    }
    let per_recipe = (thread_cpu_time() - cpu_started) / files;
    println!(
        "REFUSAL: {per_recipe:?} CPU per refused recipe ({:?} wall)",
        started.elapsed() / files
    );

    assert!(
        per_recipe < PARSE_BUDGET_PER_RECIPE,
        "refusing one recipe took {per_recipe:?} of CPU, over the {PARSE_BUDGET_PER_RECIPE:?} budget; a \
         vault of 4352 recipes would hold the boot scan for {:?}",
        per_recipe * 4352
    );
    assert!(
        per_recipe > PARSE_FLOOR_PER_RECIPE,
        "refusing one recipe took only {per_recipe:?} of CPU on this thread, under the \
         {PARSE_FLOOR_PER_RECIPE:?} floor; the parse no longer runs on the calling thread"
    );
}
