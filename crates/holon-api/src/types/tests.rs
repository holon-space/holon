use super::*;

#[test]
fn entity_name_from_str_refuses_what_names_no_entity() {
    for raw in ["", "a/b", "9lives", "a b", "**"] {
        let err = raw.parse::<EntityName>().expect_err(raw);
        assert_eq!(err.raw, raw);
    }
    assert!("*".parse::<EntityName>().unwrap().is_wildcard());
    let name: EntityName = "todoist_task".parse().unwrap();
    assert_eq!(name.as_str(), "todoist-task");
    assert_eq!(name.as_str().parse::<EntityName>().unwrap(), name);
}

#[test]
fn entity_name_deserialize_parses_and_keeps_the_wildcard() {
    let wildcard: EntityName = serde_json::from_str(r#""*""#).unwrap();
    assert!(wildcard.is_wildcard());
    let err = serde_json::from_str::<EntityName>(r#""a b""#).unwrap_err();
    assert!(
        err.to_string().contains("is not a valid entity name"),
        "{err}"
    );
}

proptest::proptest! {
    #[test]
    fn entity_name_parse_accepts_exactly_uri_schemes(raw in "[a-zA-Z0-9_+.\\- /\"*]{0,8}") {
        let normalized = raw.replace('_', "-");
        match raw.parse::<EntityName>() {
            Ok(name) if raw == "*" => proptest::prop_assert!(name.is_wildcard()),
            Ok(name) => {
                proptest::prop_assert!(is_valid_uri_scheme(&normalized));
                proptest::prop_assert_eq!(name.as_str(), normalized.as_str());
                proptest::prop_assert_eq!(name.as_str().parse::<EntityName>().unwrap(), name);
            }
            Err(_) => proptest::prop_assert!(!is_valid_uri_scheme(&normalized)),
        }
    }
}

#[test]
fn content_type_round_trip() {
    assert_eq!(ContentType::Text.to_string(), "text");
    assert_eq!(ContentType::Source.to_string(), "source");
    assert_eq!("text".parse::<ContentType>().unwrap(), ContentType::Text);
    assert_eq!(
        "source".parse::<ContentType>().unwrap(),
        ContentType::Source
    );
    assert!("invalid".parse::<ContentType>().is_err());
}

#[test]
fn content_type_value_round_trip() {
    let v: Value = ContentType::Source.into();
    assert_eq!(v, Value::String("source".into()));
    let ct: ContentType = v.try_into().unwrap();
    assert_eq!(ct, ContentType::Source);
}

#[test]
fn source_language_round_trip() {
    assert_eq!(
        "holon_prql".parse::<SourceLanguage>().unwrap(),
        SourceLanguage::Query(QueryLanguage::HolonPrql)
    );
    assert_eq!(
        "render".parse::<SourceLanguage>().unwrap(),
        SourceLanguage::Render
    );
    assert_eq!(
        "python".parse::<SourceLanguage>().unwrap(),
        SourceLanguage::Other("python".into())
    );

    assert_eq!(
        SourceLanguage::Query(QueryLanguage::HolonSql).to_string(),
        "holon_sql"
    );
    assert_eq!(SourceLanguage::Render.to_string(), "render");
    assert_eq!(SourceLanguage::Other("rust".into()).to_string(), "rust");
}

#[test]
fn source_language_rule_and_legacy() {
    // holon_rule parses to the typed rule variant and round-trips.
    assert_eq!(
        "holon_rule".parse::<SourceLanguage>().unwrap(),
        SourceLanguage::HolonRule
    );
    assert_eq!(SourceLanguage::HolonRule.to_string(), "holon_rule");
    assert!(SourceLanguage::HolonRule.is_rule());
    assert!(!SourceLanguage::HolonRule.is_legacy());

    // The retired `action` language parses to a typed deprecation sentinel —
    // NOT Other("action") — so it fails loud, never silently inert.
    let action = "action".parse::<SourceLanguage>().unwrap();
    assert_eq!(action, SourceLanguage::LegacyAction);
    assert_ne!(action, SourceLanguage::Other("action".into()));
    assert_eq!(action.to_string(), "action");
    assert!(action.is_rule());
    assert!(action.is_legacy());

    // Round-trip through serde string form.
    for lang in [SourceLanguage::HolonRule, SourceLanguage::LegacyAction] {
        let s = lang.to_string();
        assert_eq!(s.parse::<SourceLanguage>().unwrap(), lang);
    }
}

#[test]
fn task_state_done_checks() {
    assert!(TaskState::done("DONE").is_done());
    assert!(TaskState::done("CANCELLED").is_done());
    assert!(TaskState::done("CLOSED").is_done());
    assert!(!TaskState::active("TODO").is_done());
    assert!(!TaskState::active("DOING").is_done());
    assert!(!TaskState::active("WAITING").is_done());
}

#[test]
fn task_state_from_keyword_uses_defaults() {
    assert!(TaskState::from_keyword("DONE").is_done());
    assert!(TaskState::from_keyword("CANCELLED").is_done());
    assert!(TaskState::from_keyword("CLOSED").is_done());
    assert!(!TaskState::from_keyword("TODO").is_done());
    assert!(!TaskState::from_keyword("DOING").is_done());
    assert!(!TaskState::from_keyword("WAITING").is_done());
}

#[test]
fn task_state_from_keyword_with_custom_done_list() {
    let done = vec!["DONE".into(), "WONTFIX".into()];
    assert!(TaskState::from_keyword_with_done_list("WONTFIX", &done).is_done());
    assert!(TaskState::from_keyword_with_done_list("DONE", &done).is_done());
    // CANCELLED is NOT in the custom list, so it's Active
    assert!(!TaskState::from_keyword_with_done_list("CANCELLED", &done).is_done());
}

#[test]
fn task_state_round_trip() {
    assert_eq!(
        "TODO".parse::<TaskState>().unwrap(),
        TaskState::active("TODO")
    );
    assert_eq!(
        "DOING".parse::<TaskState>().unwrap(),
        TaskState::active("DOING")
    );
    assert_eq!(
        "DONE".parse::<TaskState>().unwrap(),
        TaskState::done("DONE")
    );
    assert_eq!(
        "CANCELLED".parse::<TaskState>().unwrap(),
        TaskState::done("CANCELLED")
    );
    assert_eq!(
        "WAITING".parse::<TaskState>().unwrap(),
        TaskState::active("WAITING")
    );
    assert_eq!(TaskState::active("TODO").to_string(), "TODO");
}

#[test]
fn timestamp_parse_org_format() {
    let ts = Timestamp::parse("<2026-02-21 Fri>").unwrap();
    assert_eq!(
        ts.date(),
        chrono::NaiveDate::from_ymd_opt(2026, 2, 21).unwrap()
    );
    assert_eq!(ts.as_str(), "<2026-02-21 Fri>");
}

#[test]
fn timestamp_parse_plain_date() {
    let ts = Timestamp::parse("2026-03-01").unwrap();
    assert_eq!(
        ts.date(),
        chrono::NaiveDate::from_ymd_opt(2026, 3, 1).unwrap()
    );
}

#[test]
fn timestamp_parse_with_time() {
    let ts = Timestamp::parse("<2026-02-21 Fri 10:00>").unwrap();
    assert_eq!(
        ts.date(),
        chrono::NaiveDate::from_ymd_opt(2026, 2, 21).unwrap()
    );
}

#[test]
fn timestamp_parse_invalid() {
    assert!(Timestamp::parse("not-a-date").is_err());
}

#[test]
fn tags_from_csv() {
    // Tags are a set: input is trimmed, de-duplicated, and iterated in
    // canonical (sorted) order regardless of source order.
    let tags = Tags::from_csv("foo, bar ,baz, foo");
    assert_eq!(tags.to_vec(), vec!["bar", "baz", "foo"]);
    assert_eq!(tags.to_csv(), "bar,baz,foo");
    assert_eq!(tags.to_org(), ":bar:baz:foo:");
}

#[test]
fn tags_empty() {
    let tags = Tags::from_csv("");
    assert!(tags.is_empty());
    assert_eq!(tags.to_org(), "");
    assert_eq!(tags.to_csv(), "");
}

#[test]
fn tags_from_vec() {
    let tags = Tags::from(vec!["a".into(), "b".into()]);
    assert_eq!(tags.to_csv(), "a,b");
}

#[test]
fn tags_from_str() {
    let tags: Tags = "x,y,z".parse().unwrap();
    assert_eq!(tags.to_vec(), vec!["x", "y", "z"]);
}

#[test]
fn tags_to_set() {
    let tags = Tags::from_csv("b,a,c");
    let set = tags.to_set();
    let expected: std::collections::BTreeSet<String> =
        ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
    assert_eq!(set, expected);
}

#[test]
fn priority_letter_round_trip() {
    assert_eq!("A".parse::<Priority>().unwrap(), Priority::A);
    assert_eq!(Priority::from_letter('B').unwrap(), Priority::B);
    assert_eq!(Priority::A.letter(), 'A');
    assert_eq!(Priority::C.letter(), 'C');
}

/// The whole point of the rank: 1 is the most important, so an ASCENDING
/// sort on the stored value is a sort by importance.
#[test]
fn priority_rank_ascends_with_importance() {
    assert_eq!(Priority::A.rank(), 1);
    assert_eq!(Priority::B.rank(), 2);
    assert_eq!(Priority::C.rank(), 3);
    assert_eq!(Priority::from_rank(1).unwrap(), Priority::A);
    assert_eq!(Priority::from_letter('Z').unwrap().rank(), 26);
}

/// Letters beyond the A/B/C default are DATA — org's range is configurable
/// — but a non-letter has no rank and must refuse.
#[test]
fn priority_accepts_every_letter_and_refuses_non_letters() {
    assert_eq!(Priority::from_letter('D').unwrap().rank(), 4);
    assert!(Priority::from_letter('1').is_err());
    assert!(Priority::from_letter('a').is_err());
    assert!("".parse::<Priority>().is_err());
    assert!("AB".parse::<Priority>().is_err());
}

#[test]
fn priority_rejects_invalid_rank() {
    assert!(Priority::from_rank(0).is_err());
    assert!(Priority::from_rank(27).is_err());
}

#[test]
fn priority_value_round_trip() {
    let v: Value = Priority::A.into();
    assert_eq!(v, Value::Integer(1));
    let p: Priority = v.try_into().unwrap();
    assert_eq!(p, Priority::A);

    let v: Value = Value::String("B".into());
    let p: Priority = v.try_into().unwrap();
    assert_eq!(p, Priority::B);
}

/// `Ord` IS the rank order, so sorting a `Vec<Priority>` and sorting by the
/// stored integer can never disagree.
#[test]
fn priority_ordering_matches_the_stored_rank() {
    assert!(Priority::A < Priority::B);
    assert!(Priority::B < Priority::C);
}

#[test]
fn depends_on_from_csv() {
    let deps = DependsOn::from_csv("block-1, block-2 ,block-3");
    assert_eq!(deps.as_slice(), &["block-1", "block-2", "block-3"]);
    assert_eq!(deps.to_csv(), "block-1,block-2,block-3");
}

#[test]
fn depends_on_empty() {
    let deps = DependsOn::from_csv("");
    assert!(deps.is_empty());
    assert_eq!(deps.to_csv(), "");
}

#[test]
fn depends_on_contains() {
    let deps = DependsOn::from_csv("a,b,c");
    assert!(deps.contains("b"));
    assert!(!deps.contains("d"));
}

#[test]
fn depends_on_push() {
    let mut deps = DependsOn::from_csv("a");
    deps.push("b".to_string());
    assert_eq!(deps.as_slice(), &["a", "b"]);
}

#[test]
fn depends_on_from_vec() {
    let deps = DependsOn::from(vec!["x".into(), "y".into()]);
    assert_eq!(deps.to_csv(), "x,y");
}

#[test]
fn depends_on_from_str() {
    let deps: DependsOn = "a,b,c".parse().unwrap();
    assert_eq!(deps.as_slice(), &["a", "b", "c"]);
}

#[test]
fn region_round_trip() {
    assert_eq!(Region::Main.as_str(), "main");
    assert_eq!(Region::LeftSidebar.as_str(), "left_sidebar");
    assert_eq!(Region::RightSidebar.as_str(), "right_sidebar");
    assert_eq!("main".parse::<Region>().unwrap(), Region::Main);
    assert_eq!(
        "left_sidebar".parse::<Region>().unwrap(),
        Region::LeftSidebar
    );
    assert_eq!(
        "right_sidebar".parse::<Region>().unwrap(),
        Region::RightSidebar
    );
    assert!("center".parse::<Region>().is_err());
}

#[test]
fn region_value_round_trip() {
    let v: Value = Region::Main.into();
    assert_eq!(v, Value::String("main".into()));
    let r: Region = v.try_into().unwrap();
    assert_eq!(r, Region::Main);
}

#[test]
fn region_all() {
    assert_eq!(Region::ALL.len(), 3);
    assert!(Region::ALL.contains(&Region::Main));
    assert!(Region::ALL.contains(&Region::LeftSidebar));
    assert!(Region::ALL.contains(&Region::RightSidebar));
}

/// Each rejected character, and the round-trip it breaks. Bugfunnel
/// `2026-08-30-tag-with-separator-char-loses-whole-set-on-reboot`.
#[test]
fn unrepresentable_char_names_every_separator_and_nothing_else() {
    // `,` — the CSV storage separator (`to_csv`/`from_csv`) AND outside the
    // org tag grammar: measured to drop the block's WHOLE tag set.
    assert_eq!(Tags::unrepresentable_char("a,b"), Some(','));
    // `:` — the org tag-group separator itself (`to_org` joins on it), so a
    // tag containing one SPLITS into extra tags on re-parse.
    assert_eq!(Tags::unrepresentable_char("a:b"), Some(':'));
    // Whitespace terminates the trailing tag group in org headline syntax.
    assert_eq!(Tags::unrepresentable_char("a b"), Some(' '));
    assert_eq!(Tags::unrepresentable_char("a\tb"), Some('\t'));

    // The first offender is reported, so the error can name it.
    assert_eq!(Tags::unrepresentable_char("ab,c:d"), Some(','));

    // Tags that DO round-trip today stay accepted — the org tag spec is
    // narrower (`[[:alnum:]_@#%]`) but `some-tag` and friends work, and
    // rejecting them would refuse writes that currently succeed.
    for ok in [
        "proj", "some-tag", "M", "Page", "a_b", "@home", "tag#1", "%x",
    ] {
        assert_eq!(
            Tags::unrepresentable_char(ok),
            None,
            "{ok:?} round-trips today and must not be refused"
        );
    }
}

/// The `:` claim above, demonstrated rather than asserted: a tag carrying
/// the org tag separator does not survive `to_org` -> re-parse as ONE tag.
#[test]
fn a_colon_in_a_tag_splits_the_org_tag_group() {
    let tags = Tags::from_tag_iter(vec!["a:b".to_string(), "proj".to_string()]);
    assert_eq!(tags.len(), 2);
    let rendered = tags.to_org();
    assert_eq!(rendered, ":a:b:proj:");
    // Re-reading that group yields THREE tags, not the two written: the
    // embedded `:` is indistinguishable from the separator.
    let reparsed: Vec<&str> = rendered.trim_matches(':').split(':').collect();
    assert_eq!(
        reparsed,
        vec!["a", "b", "proj"],
        "a `:` inside a tag is silently read back as a tag boundary"
    );
}

/// The `,` claim: CSV storage splits it the same way.
#[test]
fn a_comma_in_a_tag_splits_the_csv_storage_form() {
    let tags = Tags::from_tag_iter(vec!["a,b".to_string(), "proj".to_string()]);
    assert_eq!(tags.len(), 2);
    assert_eq!(Tags::from_csv(&tags.to_csv()).len(), 3);
}
