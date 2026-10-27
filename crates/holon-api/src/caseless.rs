//! The one fold under which two spellings name one file on a case- and
//! normalization-insensitive file system (APFS, the macOS default): Unicode
//! canonical caseless matching, `NFD(casefold(NFD(s)))` (Unicode §3.13,
//! D145), with full case folding (`ß` folds to `ss`, `ς` to `σ`).

use icu_casemap::CaseMapper;
use icu_normalizer::ComposingNormalizerBorrowed;
use icu_normalizer::DecomposingNormalizerBorrowed;

pub fn caseless_fold(s: &str) -> String {
    let nfd = DecomposingNormalizerBorrowed::new_nfd();
    let folded = CaseMapper::new()
        .fold_string(&nfd.normalize(s))
        .into_owned();
    nfd.normalize(&folded).into_owned()
}

/// `s` under full case folding alone, without normalizing it.
pub(crate) fn case_fold(s: &str) -> String {
    CaseMapper::new().fold_string(s).into_owned()
}

/// `s` in Unicode normalization form C.
pub(crate) fn nfc(s: &str) -> String {
    ComposingNormalizerBorrowed::new_nfc()
        .normalize(s)
        .into_owned()
}

#[cfg(test)]
mod tests;
