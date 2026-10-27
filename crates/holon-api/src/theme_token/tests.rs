use super::*;

#[test]
fn the_name_table_is_sorted_and_free_of_duplicates() {
    let mut sorted = THEME_TOKENS.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted, THEME_TOKENS.to_vec());
}

/// The names live in two places (the string table and `as_str`). This is
/// the assertion that keeps them one list.
#[test]
fn the_name_table_and_the_enum_agree() {
    let from_enum: Vec<&str> = ThemeToken::ALL.iter().map(|t| t.as_str()).collect();
    assert_eq!(from_enum, THEME_TOKENS.to_vec());
}

#[test]
fn every_token_parses_to_itself() {
    for token in ThemeToken::ALL {
        assert_eq!(ThemeToken::parse(token.as_str()).unwrap(), token);
    }
}

/// Parsing never rewrites a name. A props-only widget's fast path re-reads
/// the colour from the expression without passing the validator, so a
/// parser that canonicalised would have the two paths write different
/// strings into the same prop.
#[test]
fn parsing_never_rewrites_the_name() {
    for token in ThemeToken::ALL {
        assert_eq!(ThemeToken::parse(token.as_str()).unwrap(), token);
    }
    assert_eq!(
        ThemeToken::parse("secondary").unwrap().as_str(),
        "secondary"
    );
}

#[test]
fn an_unlisted_name_is_refused_and_the_message_names_it() {
    let err = ThemeToken::parse("corrigendum").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("\"corrigendum\""), "{msg}");
    assert!(msg.contains("THEME_TOKENS"), "{msg}");
}

#[test]
fn a_near_miss_is_named_in_the_refusal() {
    let msg = ThemeToken::parse("sucess").unwrap_err().to_string();
    assert!(msg.contains("success"), "{msg}");
}

/// The set deliberately holds no literal colour. A hex that parsed here
/// would be a colour that ignores the active theme, and this is the
/// boundary that is supposed to make that impossible.
#[test]
fn a_literal_colour_is_refused() {
    assert!(ThemeToken::parse("#3B82F6").is_err());
    assert!(ThemeToken::parse("red").is_err());
    assert!(ThemeToken::parse("gray").is_err());
}

#[test]
fn parsing_is_case_sensitive_because_the_renderers_are() {
    assert!(ThemeToken::parse("Muted").is_err());
    assert!(ThemeToken::parse("Primary").is_err());
}
