//! The yaml that declares a format, beside the `.wasm` that parses it.
//!
//! A sidecar is how a format joins the vault WITHOUT a line of wiring: the
//! registry builds a [`crate::PluginFormatAdapter`] from it. Everything the
//! host must be able to refuse — an extension, a row type, a column — is
//! declared here, so an undeclared one coming out of the guest is a refusal
//! rather than a silently-written row.

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use holon_core::file_format::WriteTier;
use serde::Deserialize;

/// Scope names the CONTRACT owns: they carry the document block and its child
/// blocks rather than declared-type rows, so a sidecar cannot claim them.
pub const DOCUMENT_SCOPE: &str = "holon.document";
pub const BLOCK_SCOPE: &str = "holon.block";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SidecarYaml {
    format: String,
    /// Path to the `.wasm` guest, relative to the sidecar.
    guest: PathBuf,
    extensions: Vec<String>,
    #[serde(default)]
    write_tier: DeclaredWriteTier,
    scopes: Vec<ScopeYaml>,
}

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum DeclaredWriteTier {
    #[default]
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScopeYaml {
    #[serde(rename = "type")]
    type_name: String,
    owner_column: String,
    columns: Vec<String>,
}

/// One declared row scope, already checked against itself.
#[derive(Debug, Clone)]
pub struct DeclaredScope {
    pub type_name: String,
    pub owner_column: String,
    pub columns: BTreeSet<String>,
    /// The URI scheme a row of this type lands under. Derived rather than
    /// declared: a type name is `snake_case` and a URI scheme cannot hold an
    /// underscore, so `ingredient_use` rows land under `ingredient-use`.
    pub id_entity: String,
}

/// A loaded, validated sidecar — the parsed form the adapter is built from.
#[derive(Debug)]
pub struct PluginFormat {
    /// Leaked because [`holon_core::file_format::FileFormatAdapter`] promises
    /// `&'static` for both. Sidecars load once at boot and live as long as the
    /// process, so the leak is bounded by the number of installed formats.
    pub format_name: &'static str,
    pub extensions: &'static [&'static str],
    pub write_tier: WriteTier,
    pub guest_path: PathBuf,
    pub scopes: Vec<DeclaredScope>,
}

impl PluginFormat {
    /// Read and validate the sidecar at `path`.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read plugin sidecar {}", path.display()))?;
        let yaml: SidecarYaml = serde_yaml::from_str(&text)
            .with_context(|| format!("plugin sidecar {} is not valid", path.display()))?;

        let dir = path.parent().with_context(|| {
            format!(
                "plugin sidecar {} has no directory to resolve its guest against",
                path.display()
            )
        })?;
        Self::from_yaml(yaml, dir)
            .with_context(|| format!("plugin sidecar {} is not admissible", path.display()))
    }

    fn from_yaml(yaml: SidecarYaml, dir: &Path) -> Result<Self> {
        if yaml.write_tier == DeclaredWriteTier::ReadWrite {
            bail!(
                "format {:?} declares write_tier: read_write, but a plugin earns write-back only \
                 by declaring a reverse export, which this contract version has no room for. A \
                 write here would render a file the plugin cannot reconstruct.",
                yaml.format
            );
        }
        if yaml.extensions.is_empty() {
            bail!(
                "format {:?} claims no extension, so no file could ever reach it",
                yaml.format
            );
        }
        for extension in &yaml.extensions {
            if extension.starts_with('.') || extension.chars().any(|c| c.is_ascii_uppercase()) {
                bail!(
                    "format {:?} claims extension {extension:?}; the watcher routes on lowercase \
                     extensions without a leading dot, so this one would claim nothing",
                    yaml.format
                );
            }
        }

        let guest_path = dir.join(&yaml.guest);
        if !guest_path.is_file() {
            bail!(
                "format {:?} names guest {}, which is not a file",
                yaml.format,
                guest_path.display()
            );
        }

        let mut scopes: Vec<DeclaredScope> = Vec::with_capacity(yaml.scopes.len());
        for scope in yaml.scopes {
            if scope.type_name == DOCUMENT_SCOPE || scope.type_name == BLOCK_SCOPE {
                bail!(
                    "format {:?} declares scope {:?}, which the contract itself owns: it carries \
                     blocks, not rows of a declared type",
                    yaml.format,
                    scope.type_name
                );
            }
            if scopes.iter().any(|s| s.type_name == scope.type_name) {
                bail!(
                    "format {:?} declares type {:?} twice",
                    yaml.format,
                    scope.type_name
                );
            }
            let columns: BTreeSet<String> = scope.columns.into_iter().collect();
            for required in ["id", scope.owner_column.as_str()] {
                if !columns.contains(required) {
                    bail!(
                        "format {:?} scope {:?} does not declare column {required:?}, which every \
                         row of it must carry",
                        yaml.format,
                        scope.type_name
                    );
                }
            }
            let id_entity = scope.type_name.replace('_', "-");
            scopes.push(DeclaredScope {
                type_name: scope.type_name,
                owner_column: scope.owner_column,
                columns,
                id_entity,
            });
        }
        if scopes.is_empty() {
            bail!(
                "format {:?} declares no row scope; a format that projects only blocks still \
                 declares them, and an empty list is indistinguishable from a truncated file",
                yaml.format
            );
        }

        let extensions: Vec<&'static str> = yaml
            .extensions
            .into_iter()
            .map(|e| &*e.leak())
            .collect::<Vec<_>>();

        Ok(Self {
            format_name: yaml.format.leak(),
            extensions: extensions.leak(),
            write_tier: WriteTier::ReadOnly,
            guest_path,
            scopes,
        })
    }

    pub fn scope(&self, type_name: &str) -> Option<&DeclaredScope> {
        self.scopes.iter().find(|s| s.type_name == type_name)
    }
}
