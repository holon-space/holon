//! Generators' source of a second spelling of a page title.

use icu_normalizer::DecomposingNormalizerBorrowed;

use crate::PageTitleKey;

/// Another spelling of `title` with its [`PageTitleKey`]: upper case
/// (`ß` → `SS`, `ς` → `Σ`) and decomposed (NFD), else with a space doubled,
/// else with a trailing space.
pub fn another_spelling(title: &str) -> String {
    let upper = DecomposingNormalizerBorrowed::new_nfd()
        .normalize(&title.to_uppercase())
        .into_owned();
    let spelling = if upper != title && PageTitleKey::of(&upper) == PageTitleKey::of(title) {
        upper
    } else if title.contains(' ') {
        title.replacen(' ', "  ", 1)
    } else {
        format!("{title} ")
    };
    assert_eq!(
        PageTitleKey::of(&spelling),
        PageTitleKey::of(title),
        "another_spelling({title:?}) = {spelling:?} names another page"
    );
    spelling
}
