//! The one fold under which two spellings name one file on a case- and
//! normalization-insensitive file system (APFS, the macOS default): Unicode
//! canonical caseless matching, `NFD(casefold(NFD(s)))` (Unicode §3.13,
//! D145), with full case folding (`ß` folds to `ss`, `ς` to `σ`).

use icu_casemap::CaseMapper;
use icu_normalizer::DecomposingNormalizerBorrowed;

pub fn caseless_fold(s: &str) -> String {
    let nfd = DecomposingNormalizerBorrowed::new_nfd();
    let folded = CaseMapper::new()
        .fold_string(&nfd.normalize(s))
        .into_owned();
    nfd.normalize(&folded).into_owned()
}

#[cfg(test)]
mod tests {
    use super::caseless_fold;

    #[test]
    fn spellings_apfs_names_one_file_fold_equal() {
        for (a, b) in [
            ("My Notes", "MY NOTES"),
            ("caf\u{e9}", "cafe\u{301}"),
            ("Stra\u{df}e", "STRASSE"),
            ("\u{3c3}", "\u{3c2}"),
            ("\u{130}stanbul", "i\u{307}stanbul"),
        ] {
            assert_eq!(caseless_fold(a), caseless_fold(b), "{a:?} / {b:?}");
        }
        assert_ne!(caseless_fold("caf\u{e9}"), caseless_fold("cafe"));
        assert_ne!(caseless_fold("My Notes"), caseless_fold("My  Notes"));
    }
}
