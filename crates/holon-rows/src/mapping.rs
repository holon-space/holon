//! A `jaq` filter, compiled once, that turns one JSON document into a row
//! stream.
//!
//! This is the mapping half the neutral contract needs at both ends: a remote
//! system's response becomes rows, and rows become that system's request body.
//! Both directions are the same operation — walk a document, emit N values —
//! so both run through one engine and one type.

use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use holon_core::file_format::TypedRowSet;
use jaq_core::Compiler;
use jaq_core::Ctx;
use jaq_core::Native;
use jaq_core::Vars;
use jaq_core::data::JustLut;
use jaq_core::load::Arena;
use jaq_core::load::File;
use jaq_core::load::Loader;
use jaq_core::unwrap_valr;
use jaq_json::Val;

/// The one value type and one function set every mapping compiles against, so
/// a filter that runs in one call site runs in all of them.
type Data = JustLut<Val>;
type Compiled = jaq_core::compile::Filter<Native<Data>>;

/// A compiled mapping.
///
/// Compilation is where a malformed filter is refused, so a mapper that exists
/// is a mapper that runs. Compiling costs about a millisecond and running costs
/// microseconds per value, so one mapper is built per declared mapping and kept
/// for the life of the connection.
pub struct RowMapper {
    /// Names the mapping in every error, because a jaq diagnostic on its own
    /// says nothing about WHICH declaration produced it.
    label: String,
    filter: Compiled,
}

impl std::fmt::Debug for RowMapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RowMapper")
            .field("label", &self.label)
            .finish()
    }
}

impl RowMapper {
    /// Compile `source` as a jaq filter over `jq`'s standard library.
    pub fn compile(label: impl Into<String>, source: &str) -> Result<Self> {
        let label = label.into();
        let loader = Loader::new(
            jaq_core::defs()
                .chain(jaq_std::defs())
                .chain(jaq_json::defs()),
        );
        let arena = Arena::default();
        let modules = loader
            .load(
                &arena,
                File {
                    code: source,
                    path: (),
                },
            )
            .map_err(|errs| filter_error(&label, source, errs))?;
        let filter = Compiler::default()
            .with_funs(
                jaq_core::funs()
                    .chain(jaq_std::funs())
                    .chain(jaq_json::funs()),
            )
            .compile(modules)
            .map_err(|errs| filter_error(&label, source, errs))?;
        Ok(Self { label, filter })
    }

    /// Run the filter over one document, collecting every value it emits.
    ///
    /// A filter that raises is an `Err` naming the mapping: a mapping that
    /// half-ran has produced a row set missing rows, which the replace-scope
    /// contract would read as deletions.
    pub fn map(&self, input: &serde_json::Value) -> Result<Vec<serde_json::Value>> {
        self.run(input)?
            .iter()
            .map(|line| {
                serde_json::from_str(line).with_context(|| {
                    format!("mapping `{}` emitted a value serde cannot read", self.label)
                })
            })
            .collect()
    }

    /// Every output value in its JSON form. `jaq` and `serde_json` share no
    /// value type, so JSON text is the bridge — and it is the same text the
    /// row-stream contract is defined over, so the row path needs no second
    /// conversion.
    fn run(&self, input: &serde_json::Value) -> Result<Vec<String>> {
        let input = jaq_json::read::parse_single(serde_json::to_string(input)?.as_bytes())
            .map_err(|e| anyhow::anyhow!("mapping `{}` cannot read its input: {e}", self.label))?;
        let ctx = Ctx::<Data>::new(&self.filter.lut, Vars::new([]));
        let mut out = Vec::new();
        for (index, result) in self
            .filter
            .id
            .run((ctx, input))
            .map(unwrap_valr)
            .enumerate()
        {
            let value = result.map_err(|e| {
                anyhow::anyhow!("mapping `{}` failed at output #{index}: {e}", self.label)
            })?;
            out.push(value.to_string());
        }
        Ok(out)
    }

    /// Run the filter and read its output as a row stream: the first value is
    /// the envelope, every later value is a row line.
    ///
    /// The values are re-serialized to the JSON Lines the contract defines and
    /// parsed back through [`crate::parse_row_sets`], so a mapping is held to
    /// exactly the rules a plugin's stream is held to — one parser, not two.
    pub fn map_to_row_sets(&self, input: &serde_json::Value) -> Result<Vec<TypedRowSet>> {
        let values = self.run(input)?;
        if values.is_empty() {
            bail!(
                "mapping `{}` emitted nothing; a row stream states its scopes on line 1 even when \
                 it carries no rows",
                self.label
            );
        }
        let mut lines = String::new();
        for value in &values {
            lines.push_str(value);
            lines.push('\n');
        }
        crate::parse_row_sets(&lines)
            .with_context(|| format!("the stream mapping `{}` produced", self.label))
    }
}

fn filter_error<E: std::fmt::Debug>(
    label: &str,
    source: &str,
    errs: Vec<(File<&str, ()>, E)>,
) -> anyhow::Error {
    let detail = errs
        .iter()
        .map(|(_, e)| format!("{e:?}"))
        .collect::<Vec<_>>()
        .join("; ");
    anyhow::anyhow!("mapping `{label}` is not a valid jaq filter: {detail}\nfilter: {source}")
}
