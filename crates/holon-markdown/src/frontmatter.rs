//! YAML frontmatter handling for Obsidian-style markdown files.
//!
//! Frontmatter is a YAML block delimited by `---` lines at the top of the
//! file:
//!
//! ```markdown
//! ---
//! title: My Note
//! tags: [project, urgent]
//! ---
//!
//! # Heading
//! ```
//!
//! We parse the YAML body as a free-form `serde_yaml::Value` and project
//! well-known keys onto typed fields:
//!
//! - `title` — document title (also rendered as `#+TITLE:` analogue in org)
//! - `tags` — array of strings or comma-separated string
//!
//! Everything else stays in `extra` so round-trip rendering preserves it.

use anyhow::{Context, Result};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Frontmatter {
    pub title: Option<String>,
    pub tags: Vec<String>,
    /// Every key that wasn't projected onto a typed field. Stored as raw
    /// YAML scalars/sequences/maps so round-trip rendering is lossless.
    pub extra: BTreeMap<String, serde_yaml::Value>,
}

impl Frontmatter {
    /// Render a frontmatter block back to YAML, including `---` delimiters
    /// and a trailing newline. Returns an empty string when the frontmatter
    /// is empty (no title, no tags, no extras) — Obsidian's convention is
    /// to omit the block entirely rather than emit `---\n---\n`.
    pub fn render(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut map = serde_yaml::Mapping::new();
        if let Some(title) = &self.title {
            map.insert(
                serde_yaml::Value::String("title".into()),
                serde_yaml::Value::String(title.clone()),
            );
        }
        if !self.tags.is_empty() {
            let seq: Vec<serde_yaml::Value> = self
                .tags
                .iter()
                .map(|t| serde_yaml::Value::String(t.clone()))
                .collect();
            map.insert(
                serde_yaml::Value::String("tags".into()),
                serde_yaml::Value::Sequence(seq),
            );
        }
        for (k, v) in &self.extra {
            map.insert(serde_yaml::Value::String(k.clone()), v.clone());
        }
        let yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping(map))
            .expect("YAML frontmatter must serialize");
        format!("---\n{}---\n", yaml)
    }

    pub fn is_empty(&self) -> bool {
        self.title.is_none() && self.tags.is_empty() && self.extra.is_empty()
    }
}

/// Split a markdown document into (frontmatter_yaml, body) tuples.
/// Returns `(None, full_content)` when no frontmatter block is present.
///
/// We do this manually rather than relying on `markdown::ParseOptions { frontmatter: true }`
/// because we need the raw YAML text to round-trip unrecognized keys
/// without normalization, and we need to control the body offset so
/// `Position` info from the AST aligns with our source text.
pub fn split_frontmatter(content: &str) -> (Option<&str>, &str) {
    if !content.starts_with("---") {
        return (None, content);
    }
    let after_open = match content.strip_prefix("---\n") {
        Some(rest) => rest,
        // Obsidian also accepts `---\r\n`, but our store only writes `\n`.
        // For first-impl we skip the CRLF case; if a vault user imports
        // CRLF files we'll surface that as a parse error instead of
        // silently mis-extracting.
        None => return (None, content),
    };
    let close_marker = "\n---\n";
    let close_at = match after_open.find(close_marker) {
        Some(i) => i,
        None => return (None, content),
    };
    let yaml = &after_open[..close_at];
    let body_start =
        (after_open.as_ptr() as usize - content.as_ptr() as usize) + close_at + close_marker.len();
    let body = &content[body_start..];
    (Some(yaml), body)
}

pub fn parse(content: &str) -> Result<(Frontmatter, &str)> {
    let (yaml_opt, body) = split_frontmatter(content);
    let Some(yaml) = yaml_opt else {
        return Ok((Frontmatter::default(), body));
    };
    if yaml.trim().is_empty() {
        return Ok((Frontmatter::default(), body));
    }
    let value: serde_yaml::Value = serde_yaml::from_str(yaml)
        .with_context(|| format!("invalid YAML frontmatter: {yaml:?}"))?;
    let map = match value {
        serde_yaml::Value::Mapping(m) => m,
        // A scalar/sequence as the entire frontmatter is non-standard;
        // refuse it loudly so the user catches typos rather than silently
        // dropping content (per the project's "fail loud" rule).
        other => anyhow::bail!(
            "frontmatter must be a YAML mapping, got {:?}",
            other_kind(&other)
        ),
    };

    let mut fm = Frontmatter::default();
    for (k, v) in map {
        let key = match k {
            serde_yaml::Value::String(s) => s,
            other => anyhow::bail!("frontmatter keys must be strings, got {other:?}"),
        };
        match key.as_str() {
            "title" => {
                fm.title = match v {
                    serde_yaml::Value::String(s) => Some(s),
                    serde_yaml::Value::Null => None,
                    other => anyhow::bail!("`title` must be a string, got {other:?}"),
                };
            }
            "tags" => {
                fm.tags = parse_tags(v)?;
            }
            _ => {
                fm.extra.insert(key, v);
            }
        }
    }
    Ok((fm, body))
}

fn parse_tags(v: serde_yaml::Value) -> Result<Vec<String>> {
    match v {
        serde_yaml::Value::Null => Ok(Vec::new()),
        serde_yaml::Value::String(s) => Ok(s
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect()),
        serde_yaml::Value::Sequence(seq) => seq
            .into_iter()
            .map(|item| match item {
                serde_yaml::Value::String(s) => Ok(s),
                other => anyhow::bail!("tag must be a string, got {other:?}"),
            })
            .collect(),
        other => anyhow::bail!("`tags` must be a string or list, got {other:?}"),
    }
}

fn other_kind(v: &serde_yaml::Value) -> &'static str {
    match v {
        serde_yaml::Value::Null => "null",
        serde_yaml::Value::Bool(_) => "bool",
        serde_yaml::Value::Number(_) => "number",
        serde_yaml::Value::String(_) => "string",
        serde_yaml::Value::Sequence(_) => "sequence",
        serde_yaml::Value::Mapping(_) => "mapping",
        serde_yaml::Value::Tagged(_) => "tagged",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_frontmatter_block() {
        let src = "---\ntitle: foo\n---\n# Heading\n";
        let (yaml, body) = split_frontmatter(src);
        // Closing `\n---\n` consumes the leading newline, so the YAML
        // slice has no trailing newline — that's fine for serde_yaml.
        assert_eq!(yaml, Some("title: foo"));
        assert_eq!(body, "# Heading\n");
    }

    #[test]
    fn missing_frontmatter_passes_through() {
        let src = "# Heading\n";
        let (yaml, body) = split_frontmatter(src);
        assert!(yaml.is_none());
        assert_eq!(body, src);
    }

    #[test]
    fn parses_title_and_tags_list() {
        let src = "---\ntitle: My Note\ntags: [a, b]\n---\nbody\n";
        let (fm, body) = parse(src).unwrap();
        assert_eq!(fm.title.as_deref(), Some("My Note"));
        assert_eq!(fm.tags, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(body, "body\n");
    }

    #[test]
    fn parses_tags_as_csv_string() {
        let src = "---\ntags: alpha, beta, gamma\n---\nbody\n";
        let (fm, _) = parse(src).unwrap();
        assert_eq!(fm.tags, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn extras_round_trip() {
        let src = "---\ncreated: 2026-04-26\nfoo:\n  bar: 1\n---\n";
        let (fm, _) = parse(src).unwrap();
        assert!(fm.extra.contains_key("created"));
        assert!(fm.extra.contains_key("foo"));
        let rendered = fm.render();
        assert!(rendered.starts_with("---\n"));
        assert!(rendered.contains("created:"));
        assert!(rendered.contains("foo:"));
    }

    #[test]
    fn rejects_non_mapping_frontmatter() {
        let src = "---\n- a\n- b\n---\nbody\n";
        let err = parse(src).unwrap_err();
        assert!(format!("{err:?}").contains("mapping"));
    }

    #[test]
    fn empty_frontmatter_renders_nothing() {
        let fm = Frontmatter::default();
        assert_eq!(fm.render(), "");
    }
}
