//! Which profile governs a block's HOME.
//!
//! The binding is DERIVED on every read, never stored: a column holding it
//! would be a second answer, free to disagree with the home the authority
//! reports now.

use anyhow::Result;
use holon_api::live_data::home_by::DurableFormat;
use holon_api::live_data::home_by::Home;
use holon_api::live_data::home_by::HomeDoc;

use crate::profile::CapabilityProfileId;

/// The profile id of the home `home` currently sits in.
///
/// Pure and total: it names a profile, and [`crate::ProfileRegistry::get`] is
/// what turns that name into the profile's data. Handing back the profile
/// VALUE here would let a caller keep answering from a home the block has
/// since left.
pub fn profile_for<K: HomeDoc>(home: &Home<K>) -> Result<CapabilityProfileId> {
    Ok(profile_of(home.doc.durable_format()?))
}

/// The profile id of a home already read down to its durable format.
///
/// The mapping [`profile_for`] applies, for a caller that holds the format
/// rather than the home — an operation reporting which homes it moved between.
pub fn profile_of(format: Option<DurableFormat>) -> CapabilityProfileId {
    let id = match format {
        Some(DurableFormat::Org) => "org",
        None => "holon-native",
    };
    CapabilityProfileId::new(id)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn resolve(doc: Option<&str>) -> Result<CapabilityProfileId> {
        profile_for(&Home {
            doc: doc.map(PathBuf::from),
            prev: None,
        })
    }

    /// The return type IS the assertion: a caller receives a name it must take
    /// back to the registry, never profile data it can cache.
    #[test]
    fn the_resolver_answers_with_a_profile_id() {
        let id: CapabilityProfileId = resolve(Some("Notes.org")).expect("an org home resolves");
        assert_eq!(id, CapabilityProfileId::new("org"));
    }

    #[test]
    fn a_home_no_file_holds_is_holon_native() {
        assert_eq!(
            resolve(None).expect("a fileless home resolves"),
            CapabilityProfileId::new("holon-native")
        );
    }

    #[test]
    fn a_file_no_profile_covers_is_an_error() {
        let err = resolve(Some("Notes.md"))
            .expect_err("`.md` has no profile")
            .to_string();
        assert!(
            err.contains("md"),
            "the error must name the extension: {err}"
        );
    }

    #[test]
    fn a_home_file_without_an_extension_is_an_error() {
        let err = resolve(Some("Notes"))
            .expect_err("an extensionless home names no format")
            .to_string();
        assert!(err.contains("Notes"), "the error must name the file: {err}");
    }
}
