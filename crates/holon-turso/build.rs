//! Names the `turso_core` build this crate links, so an open can tell views
//! another engine build planned from views this one planned.

use std::collections::BTreeSet;
use std::path::PathBuf;

fn main() {
    // shortcut: the workspace lock is read for every build, including the
    // frontends that keep their own lock; they pin the same rev, and a
    // frontend that moves its pin alone needs its own lock read here.
    let lock = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo"))
        .join("../../Cargo.lock");
    println!("cargo::rerun-if-changed={}", lock.display());
    let text =
        std::fs::read_to_string(&lock).unwrap_or_else(|e| panic!("read {}: {e}", lock.display()));
    let revs: BTreeSet<&str> = text
        .split("[[package]]")
        .filter(|package| package.lines().any(|l| l == "name = \"turso_core\""))
        .map(|package| {
            let source = package
                .lines()
                .find_map(|l| l.strip_prefix("source = \""))
                .unwrap_or_else(|| panic!("{}: turso_core has no source line", lock.display()));
            let (_, rev) = source
                .trim_end_matches('"')
                .rsplit_once('#')
                .unwrap_or_else(|| {
                    panic!(
                        "{}: turso_core source [{source}] names no commit",
                        lock.display()
                    )
                });
            rev
        })
        .collect();
    let [rev] = revs.iter().collect::<Vec<_>>()[..] else {
        panic!(
            "{}: expected one turso_core commit, found {revs:?}",
            lock.display()
        );
    };
    assert!(
        rev.len() == 40 && rev.bytes().all(|b| b.is_ascii_hexdigit()),
        "{}: turso_core commit [{rev}] is not a full git hash",
        lock.display()
    );
    println!("cargo::rustc-env=HOLON_TURSO_CORE_COMMIT={rev}");
}
