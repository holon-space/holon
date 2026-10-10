//! Conflict copies: a vault file's text saved beside it before a write-back
//! overrules part of it, so the user's text survives on disk.
//!
//! A copy is named `<stem>.conflict-<UTC YYYYMMDDTHHMMSSZ>-<NNN>.<ext>`.
//! It is never a vault document: [`crate::file_format::FormatRegistry`]
//! claims no conflict copy, so no scan or watcher ingests one, and nothing
//! deletes one.

use std::path::Path;
use std::path::PathBuf;

use chrono::DateTime;

const MARKER: &str = ".conflict-";
const STAMP_FORMAT: &str = "%Y%m%dT%H%M%SZ";
/// `YYYYMMDDTHHMMSSZ`
const STAMP_LEN: usize = 16;
const SEQ_LEN: usize = 3;
/// The number of distinct copy names per file and second.
pub const SEQ_COUNT: u16 = 1000;

/// Whether `path` is named as a conflict copy.
pub fn is_conflict_copy(path: &Path) -> bool {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return false;
    };
    if path.extension().is_none() {
        return false;
    }
    let Some(at) = stem.rfind(MARKER) else {
        return false;
    };
    if at == 0 {
        return false;
    }
    let suffix = &stem[at + MARKER.len()..];
    let Some((stamp, seq)) = suffix.split_once('-') else {
        return false;
    };
    stamp.len() == STAMP_LEN
        && chrono::NaiveDateTime::parse_from_str(stamp, STAMP_FORMAT).is_ok()
        && seq.len() == SEQ_LEN
        && seq.bytes().all(|b| b.is_ascii_digit())
}

/// The conflict copy name for `path` at `millis` (Unix epoch, UTC) with
/// sequence number `seq`, which tells apart copies made in one second.
pub fn path_for(path: &Path, millis: i64, seq: u16) -> PathBuf {
    assert!(seq < SEQ_COUNT, "conflict copy sequence {seq} out of range");
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_else(|| panic!("conflict copy of a path without a stem: {}", path.display()));
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_else(|| {
            panic!(
                "conflict copy of a path without an extension: {}",
                path.display()
            )
        });
    let at = DateTime::from_timestamp_millis(millis)
        .unwrap_or_else(|| panic!("clock millis {millis} out of range"));
    path.with_file_name(format!(
        "{stem}{MARKER}{}-{seq:03}.{ext}",
        at.format(STAMP_FORMAT)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_name_is_recognised_and_its_source_is_not() {
        let source = Path::new("/vault/Notes.org");
        let copy = path_for(source, 1_760_000_000_123, 7);
        assert_eq!(
            copy,
            Path::new("/vault/Notes.conflict-20251009T085320Z-007.org")
        );
        assert!(is_conflict_copy(&copy));
        assert!(!is_conflict_copy(source));
    }

    #[test]
    fn only_the_strict_pattern_is_a_copy() {
        for name in [
            "Notes.conflict-.org",
            "Notes.conflict-20251009T085320Z.org",
            "Notes.conflict-20251009T085320Z-7.org",
            "Notes.conflict-2025100T085320Z-007.org",
            "Notes.conflict-20251399T085320Z-007.org",
            "Notes.conflict-20251009T085320Z-007",
            ".conflict-20251009T085320Z-007.org",
            "conflict-20251009T085320Z-007.org",
        ] {
            assert!(!is_conflict_copy(Path::new(name)), "{name}");
        }
        assert!(is_conflict_copy(Path::new(
            "a.conflict-x.conflict-20251009T085320Z-999.md"
        )));
    }
}
