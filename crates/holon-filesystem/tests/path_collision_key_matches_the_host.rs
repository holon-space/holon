//! `PathCollisionKey` against the real file system of this host: two
//! spellings share a key exactly when writing the first and then probing the
//! second reaches the same file. On macOS (APFS) that is case- and
//! normalization-insensitive with full case folding; on any host a collision
//! the file system makes must be one the key makes.

use std::path::Path;

use holon_filesystem::PathCollisionKey;

const PAIRS: &[(&str, &str)] = &[
    ("My Notes", "my notes"),
    ("caf\u{e9}", "cafe\u{301}"),
    ("\u{e4}", "a\u{308}"),
    ("Strasse", "Stra\u{df}e"),
    ("\u{3c3}", "\u{3c2}"),
    ("\u{130}stanbul", "i\u{307}stanbul"),
    ("\u{c4}pfel", "\u{e4}pfel"),
    ("My Notes", "My  Notes"),
    ("caf\u{e9}", "cafe"),
    ("Notes", "Notes2"),
];

fn host_collides(dir: &Path, a: &str, b: &str) -> bool {
    let first = dir.join(format!("{a}.org"));
    std::fs::write(&first, a).unwrap();
    let collides = dir.join(format!("{b}.org")).exists();
    std::fs::remove_file(&first).unwrap();
    collides
}

#[test]
fn path_collision_key_names_one_file_exactly_where_the_host_does() {
    let tmp = tempfile::tempdir().unwrap();
    let mut wrong = Vec::new();
    for (a, b) in PAIRS {
        let host = host_collides(tmp.path(), a, b);
        let key = PathCollisionKey::of(&tmp.path().join(format!("{a}.org")))
            == PathCollisionKey::of(&tmp.path().join(format!("{b}.org")));
        let agrees = if cfg!(target_os = "macos") {
            host == key
        } else {
            !host || key
        };
        if !agrees {
            wrong.push(format!(
                "{a:?} / {b:?}: one file on this host = {host}, one key = {key}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "PathCollisionKey disagrees with the host file system:\n{}",
        wrong.join("\n")
    );
}
