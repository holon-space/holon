use super::*;

#[test]
fn the_table_is_sorted_and_free_of_duplicates() {
    let mut sorted = ICON_NAMES.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted, ICON_NAMES.to_vec());
}

#[test]
fn a_listed_name_parses_to_itself() {
    assert_eq!(IconName::parse("robot").unwrap().as_str(), "robot");
    assert_eq!(IconName::parse("link").unwrap().as_str(), "link");
}

#[test]
fn an_unlisted_name_is_refused_and_the_message_names_it() {
    let err = IconName::parse("plug").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("\"plug\""), "{msg}");
    assert!(msg.contains("ICON_NAMES"), "{msg}");
}

#[test]
fn parsing_is_case_sensitive_because_the_renderer_is() {
    assert!(IconName::parse("Robot").is_err());
}
