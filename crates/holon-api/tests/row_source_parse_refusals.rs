//! The `live_query` builder must fail LOUDLY on a bad row source, never render
//! an empty collection: an empty list is indistinguishable from "the source is
//! empty", which is the silent failure the typed spec exists to remove.

use std::sync::Arc;

use holon_api::interp_value::ReactiveRowProvider;
use holon_api::row_source::NamedSourceDef;
use holon_api::row_source::RowSource;
use holon_api::row_source::RowSourceError;
use holon_api::row_source::RowSourceRegistry;
use holon_api::row_source::RowSourceSpec;

struct Stub;

impl RowSource for Stub {
    fn def(&self) -> NamedSourceDef {
        NamedSourceDef {
            name: "stub",
            columns: &["id", "state"],
        }
    }
    fn provider(
        &self,
        _filter: Option<&holon_api::row_source::RowFilter>,
    ) -> Arc<dyn ReactiveRowProvider> {
        unreachable!("parse tests never build a provider")
    }
}

fn registry() -> RowSourceRegistry {
    let mut r = RowSourceRegistry::new();
    r.register(Arc::new(Stub)).expect("first registration");
    r
}

#[test]
fn an_unknown_source_is_an_error_naming_what_is_registered() {
    let err = registry()
        .parse_named("condiitons", None)
        .expect_err("a typo must not parse");
    let RowSourceError::UnknownSource { asked, known } = &err else {
        panic!("expected UnknownSource, got {err:?}");
    };
    assert_eq!(asked, "condiitons");
    assert_eq!(known, &vec!["stub"]);
    assert!(
        err.to_string().contains("registered sources: stub"),
        "the message must name what IS available: {err}"
    );
}

#[test]
fn a_filter_over_an_undeclared_column_is_an_error() {
    let err = registry()
        .parse_named("stub", Some(("staet", "failing")))
        .expect_err("a column the source does not produce must not parse");
    let RowSourceError::UnknownColumn {
        source,
        asked,
        declared,
    } = &err
    else {
        panic!("expected UnknownColumn, got {err:?}");
    };
    assert_eq!(*source, "stub");
    assert_eq!(asked, "staet");
    assert_eq!(declared, &vec!["id", "state"]);
}

#[test]
fn a_declared_column_parses_and_carries_the_registrys_own_name() {
    let spec = registry()
        .parse_named("stub", Some(("state", "failing")))
        .expect("a declared column parses");
    let RowSourceSpec::Named(named) = spec else {
        panic!("expected a Named spec");
    };
    assert_eq!(named.name().as_str(), "stub");
    let filter = named.filter().expect("the filter survives the parse");
    assert_eq!(filter.column().as_str(), "state");
    assert_eq!(filter.equals(), "failing");
}

#[test]
fn registering_one_name_twice_is_an_error_not_a_silent_overwrite() {
    let mut r = registry();
    let err = r
        .register(Arc::new(Stub))
        .expect_err("a second registration under the same name must be refused");
    assert_eq!(err, RowSourceError::DuplicateRegistration { name: "stub" });
}
