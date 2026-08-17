//! The sidecars this build ships, compiled in.
//!
//! An installed file under `{config_dir}/integrations/` carries two jobs that
//! do not belong together: it says WHICH providers are on, and it supplies
//! their content. Conflating them makes every installed copy a private fork of
//! a schema that keeps evolving in the repo, with nothing to notice the drift.
//!
//! Here the two are split: an installed file still enables its provider, but
//! for a provider the build knows, the CONTENT comes from this bundle unless
//! the installed file declares [`SIDECAR_SCHEMA_VERSION`] (see
//! [`crate::integration_config::load_integration_configs`]).

/// Schema generation of the sidecar format this build understands.
///
/// Bump it whenever a sidecar-format requirement lands that a previously valid
/// file does not satisfy — that is exactly the event that turns every installed
/// copy into a stale one, and the bump is what makes it say so.
pub const SIDECAR_SCHEMA_VERSION: u32 = 1;

/// One compiled-in sidecar.
pub struct BundledSidecar {
    /// Provider name — the installed file's stem must match it.
    pub provider: &'static str,
    /// Repo-relative path of the source, quoted in disclosures so the message
    /// names something a reader can actually open.
    pub source_path: &'static str,
    pub yaml: &'static str,
}

macro_rules! bundled {
    ($provider:literal) => {
        BundledSidecar {
            provider: $provider,
            source_path: concat!("assets/integrations/", $provider, ".yaml"),
            yaml: include_str!(concat!("../../../assets/integrations/", $provider, ".yaml")),
        }
    };
}

pub static BUNDLED_SIDECARS: &[BundledSidecar] = &[
    bundled!("claude-history"),
    bundled!("gcal"),
    bundled!("gmail"),
    bundled!("jsonplaceholder"),
    bundled!("todoist"),
];

/// The sidecar this build ships for `provider`, if it ships one.
pub fn bundled_sidecar(provider: &str) -> Option<&'static BundledSidecar> {
    BUNDLED_SIDECARS.iter().find(|s| s.provider == provider)
}
