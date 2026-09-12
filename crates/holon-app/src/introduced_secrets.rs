//! The credentials connections INTRODUCED by user files ask for.
//!
//! The Settings secret fields were a compile-time list of two keys, written
//! when every connection was compiled in. A connection a user installs names
//! its own `${VAR}`s, so that list could never hold a field for one — it could
//! be switched on and connect, with nowhere in the UI to type its token. This
//! module derives those fields from the roster.
//!
//! It sits in `holon-app` because reading the integrations directory is the
//! composition root's job: `holon-frontend` turns the result into settings
//! fields and knows nothing about sidecars.

use std::path::Path;

use holon_frontend::preferences::IntroducedSecret;

/// Every `${VAR}` the introduced connections in `dir` reference, de-duplicated
/// per connection and in the order the file names them.
///
/// Bundled connections are skipped: their fields are declared in
/// `define_preferences`, and deriving them here as well would give one
/// credential two fields.
///
/// A roster scan this cannot complete is disclosed and treated as an empty
/// directory. The loader reports the same unreadable directory on the degraded
/// bus, and a Settings modal that refused to render would take away the surface
/// for switching the offending connection off.
pub fn introduced_secret_fields(dir: &Path) -> Vec<IntroducedSecret> {
    let roster = match holon_mcp_client::ConnectionRoster::scan(dir) {
        Ok(roster) => roster,
        Err(e) => {
            tracing::warn!(
                "[introduced_secrets] could not scan '{}' ({e:#}); Settings offers no credential \
                 field for any introduced connection this boot",
                dir.display()
            );
            return Vec::new();
        }
    };

    let mut out = Vec::new();
    for entry in roster.entries() {
        let holon_mcp_client::ConnectionSource::Installed { path } = &entry.source else {
            continue;
        };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) => {
                tracing::warn!(
                    "[introduced_secrets] '{}' named connection '{}' during the scan but will not \
                     read now ({e}); Settings offers no credential field for it",
                    path.display(),
                    entry.name
                );
                continue;
            }
        };
        // Only the connection's OWN namespace. A reference outside it is what
        // `check_secret_namespace` refuses the whole connection for, so a field
        // for it would offer to fill in a credential this build will not use —
        // and, because the prefix is what keeps derived keys apart, a foreign
        // reference is exactly what can collide with a bundled key and take the
        // boot down (`${TODOIST_API_KEY}` in an installed file).
        let prefix = holon_mcp_client::roster::secret_namespace_prefix(&entry.name);
        let mut seen = std::collections::HashSet::new();
        for var in holon_mcp_client::roster::referenced_vars(&text) {
            if !var
                .to_ascii_lowercase()
                .replace(['.', '-'], "_")
                .starts_with(&prefix)
            {
                continue;
            }
            if !seen.insert(var.clone()) {
                continue;
            }
            out.push(IntroducedSecret {
                connection: entry.name.to_string(),
                origin: path.display().to_string(),
                var,
            });
        }
    }
    out
}
