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

use crate::support;

/// Twice the size of the largest recipe that parses in a real corpus, as the
/// stand-in for the largest legitimate file: a 4x fuel margin over it keeps
/// every real file clear of a limit meant for a runaway guest.
const LARGEST_LEGITIMATE_RECIPE_BYTES: usize = 2 * 11_049;

#[test]
fn fuel_and_memory_headroom() {
    let wasm = std::fs::read(support::plugins_dir().join("cooklang.wasm")).unwrap();
    let limits = PluginLimits::default();
    let mut host = PluginHost::from_bytes(&wasm, limits).unwrap();

    let recipe = (1..)
        .map(support::big_recipe)
        .find(|r| r.len() >= LARGEST_LEGITIMATE_RECIPE_BYTES)
        .unwrap();
    host.parse(recipe.as_bytes(), LARGE_CTX)
        .expect("a recipe twice the largest real one is an ordinary parse");

    let spent = limits.fuel_per_call - host.fuel_remaining();
    let memory = host.memory_bytes();
    println!(
        "{}-byte recipe: {spent} fuel, {memory} bytes of guest memory",
        recipe.len()
    );

    assert!(
        spent * 4 < limits.fuel_per_call,
        "the fuel budget must leave a 4x margin over twice the largest real recipe: {spent} of {}",
        limits.fuel_per_call
    );
    assert!(
        memory * 4 < limits.memory_bytes,
        "the memory budget must leave a 4x margin: {memory} of {}",
        limits.memory_bytes
    );
}

const LARGE_CTX: &[u8] =
    br#"{"source_path":"Rezepte/Large.cook","file_stem":"Large","sibling_files":[]}"#;

/// A 200-step recipe is far larger than any real one; it must still parse
/// within the limits.
#[test]
fn a_200_step_recipe_parses_within_the_limits() {
    let wasm = std::fs::read(support::plugins_dir().join("cooklang.wasm")).unwrap();
    let limits = PluginLimits::default();
    let mut host = PluginHost::from_bytes(&wasm, limits).unwrap();

    host.parse(support::big_recipe(200).as_bytes(), LARGE_CTX)
        .expect("a 200-step recipe is an ordinary parse");
    println!(
        "200-step recipe: {} fuel, {} bytes of guest memory",
        limits.fuel_per_call - host.fuel_remaining(),
        host.memory_bytes()
    );
}

/// The fuel each `.cook` file under `$HOLON_COOK_CORPUS` spends on a warm host,
/// reported as max and p99. The corpus is private data, so the test reads it
/// from the environment instead of the repository.
#[test]
#[ignore = "measurement over a private corpus; set HOLON_COOK_CORPUS"]
fn fuel_over_a_recipe_corpus() {
    let corpus = PathBuf::from(
        std::env::var("HOLON_COOK_CORPUS").expect("HOLON_COOK_CORPUS names the corpus directory"),
    );
    let wasm = std::fs::read(support::plugins_dir().join("cooklang.wasm")).unwrap();
    let limits = PluginLimits::default();
    let mut host = PluginHost::from_bytes(&wasm, limits).unwrap();

    let mut files = vec![corpus];
    let mut spent = Vec::new();
    let mut refused = 0usize;
    while let Some(path) = files.pop() {
        if path.is_dir() {
            files.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("cook") {
            continue;
        }
        let content = std::fs::read(&path).unwrap();
        match host.parse(&content, LARGE_CTX) {
            Ok(_) => spent.push((limits.fuel_per_call - host.fuel_remaining(), content.len())),
            Err(e) => {
                assert!(
                    !format!("{e:#}").contains("fuel"),
                    "{} ran out of fuel: {e:#}",
                    path.display()
                );
                println!("REFUSED: {e:#}");
                refused += 1;
            }
        }
    }
    assert!(
        !spent.is_empty(),
        "the corpus holds no parseable .cook file"
    );
    spent.sort_unstable();
    let (max, max_bytes) = spent[spent.len() - 1];
    let largest = spent.iter().map(|(_, bytes)| *bytes).max().unwrap();
    let p99 = spent[(spent.len() * 99).div_ceil(100) - 1].0;
    println!(
        "CORPUS FUEL: {} parsed, {refused} refused; max {max} ({max_bytes} bytes), p99 {p99}; \
         largest file {largest} bytes; cap {}",
        spent.len(),
        limits.fuel_per_call
    );
}

/// What a vault scan pays per file once the host is warm. Measured with the
/// unit table built once and the talc allocator: 2.4 M for a real recipe and
/// 139.4 M for the 200-step one; rebuilding the unit table on every call cost
/// 5.25 M per file on top.
#[test]
fn a_warm_host_spends_fuel_on_the_recipe_rather_than_on_setup() {
    let wasm = std::fs::read(support::plugins_dir().join("cooklang.wasm")).unwrap();
    let limits = PluginLimits::default();
    let mut host = PluginHost::from_bytes(&wasm, limits).unwrap();
    let ctx =
        br#"{"source_path":"Rezepte/Pancakes.cook","file_stem":"Pancakes","sibling_files":[]}"#;
    let mut spend = |recipe: &str| {
        host.parse(recipe.as_bytes(), ctx)
            .expect("an ordinary recipe parses");
        limits.fuel_per_call - host.fuel_remaining()
    };

    let real = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pancakes.cook"),
    )
    .unwrap();
    spend(&real);
    let real_spent = spend(&real);
    let big_spent = spend(&support::big_recipe(200));
    println!("WARM: pancakes.cook {real_spent} fuel, 200-step recipe {big_spent} fuel");

    assert!(
        real_spent < 4_000_000,
        "a {}-byte recipe spent {real_spent} fuel on a warm host; per-call setup is back",
        real.len()
    );
    assert!(
        big_spent < 175_000_000,
        "the 200-step recipe spent {big_spent} fuel on a warm host"
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

/// The boot scan parses every `.cook` file in the vault on one thread. Holon is
/// dogfooded on debug builds, so this budget holds in the profile the tests run
/// in, not only in release. It is CPU time on the parsing thread, so machine
/// load does not move it.
const PARSE_BUDGET_PER_RECIPE: Duration = Duration::from_millis(20);

/// Measured cost is 4-5 ms. A reading far below that, while the guest ran
/// once per file, means it ran on another thread and the budget bounds nothing.
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

/// A path with an empty segment names no document whatever the recipe says, so
/// refusing it may not cost a guest run.
#[test]
fn a_recipe_whose_path_cannot_be_stored_is_refused_before_the_guest_runs() {
    let plugin = support::bundled_cook_plugin();
    let root = PathBuf::from("/vault");
    let recipe = support::big_recipe(20);
    let files = 20;

    for (name, reason) in [("Rezepte//Brot", "names no document")] {
        let parses_before = holon_plugin_host::guest_parses();
        let started = Instant::now();
        let cpu_started = thread_cpu_time();
        for i in 0..files {
            let path = root.join(format!("{name}-{i}.cook"));
            let Err(refusal) = plugin.parse(&path, &recipe, &EntityUri::no_parent(), &root) else {
                panic!("{} derives an unstorable recipe id", path.display());
            };
            let refusal = format!("{refusal:#}");
            assert!(
                refusal.contains(reason),
                "refused for another reason: {refusal}"
            );
            if i == 0 {
                println!("REFUSAL MESSAGE: {refusal}");
            }
        }
        println!(
            "REFUSAL: {:?} CPU per refused recipe ({:?} wall)",
            (thread_cpu_time() - cpu_started) / files,
            started.elapsed() / files
        );
        let parses = holon_plugin_host::guest_parses() - parses_before;
        assert_eq!(
            parses, 0,
            "refusing {files} recipes named {name:?} ran the guest {parses} times; their ids \
             derive from the path alone"
        );
    }
}

#[test]
fn parsing_a_recipe_stays_within_the_scan_budget_in_every_build_profile() {
    let plugin = support::bundled_cook_plugin();
    let root = PathBuf::from("/vault");
    let recipe = support::big_recipe(20);
    let files = 20;

    let parses_before = holon_plugin_host::guest_parses();
    let started = Instant::now();
    let cpu_started = thread_cpu_time();
    for i in 0..files {
        let path = root.join(format!("Rezepte/Aargauer-Rueeblitorte-{i}.cook"));
        plugin
            .parse(&path, &recipe, &EntityUri::no_parent(), &root)
            .expect("an ordinary recipe at a storable path parses");
    }
    let per_recipe = (thread_cpu_time() - cpu_started) / files;
    println!(
        "PARSE: {per_recipe:?} CPU per recipe ({:?} wall)",
        started.elapsed() / files
    );

    assert!(
        per_recipe < PARSE_BUDGET_PER_RECIPE,
        "parsing one recipe took {per_recipe:?} of CPU, over the {PARSE_BUDGET_PER_RECIPE:?} \
         budget; a vault of 4352 recipes would hold the boot scan for {:?}",
        per_recipe * 4352
    );
    let parses = holon_plugin_host::guest_parses() - parses_before;
    assert_eq!(parses, files as u64, "the guest must run once per recipe");
    assert!(
        per_recipe > PARSE_FLOOR_PER_RECIPE,
        "the guest ran {parses} times but this thread spent only {per_recipe:?} per recipe, under \
         the {PARSE_FLOOR_PER_RECIPE:?} floor; the parse no longer runs on the calling thread"
    );
}
