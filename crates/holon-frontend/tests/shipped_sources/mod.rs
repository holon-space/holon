//! Every render source Holon ships: the kitchen profiles, every `render:` in a
//! yaml under `assets/default` and `assets/integrations`, and every
//! `#+BEGIN_SRC render` block of an org file there, as the org parser reads it.

use std::path::Path;
use std::path::PathBuf;

use holon_api::EntityUri;
use holon_api::SourceLanguage;

fn collect_yaml_renders(node: &serde_yaml::Value, out: &mut Vec<String>) {
    match node {
        serde_yaml::Value::Mapping(map) => {
            for (k, v) in map {
                match (k.as_str(), v.as_str()) {
                    (Some("render"), Some(render)) => out.push(render.to_string()),
                    _ => collect_yaml_renders(v, out),
                }
            }
        }
        serde_yaml::Value::Sequence(items) => {
            items.iter().for_each(|v| collect_yaml_renders(v, out))
        }
        _ => {}
    }
}

fn yaml_renders(label: &str, yaml: &str) -> Vec<(String, String)> {
    let doc: serde_yaml::Value =
        serde_yaml::from_str(yaml).unwrap_or_else(|e| panic!("{label} is not yaml: {e}"));
    let mut renders = Vec::new();
    collect_yaml_renders(&doc, &mut renders);
    renders
        .into_iter()
        .enumerate()
        .map(|(i, r)| (format!("{label}#{i}"), r))
        .collect()
}

fn org_renders(label: &str, org: &str) -> Vec<(String, String)> {
    let parsed = holon_org_format::parse_org_file(
        Path::new(label),
        org,
        &EntityUri::no_parent(),
        Path::new(""),
    )
    .unwrap_or_else(|e| panic!("{label} does not parse: {e:#}"));
    parsed
        .blocks
        .into_iter()
        .filter(|b| matches!(b.source_language, Some(SourceLanguage::Render)))
        .enumerate()
        .map(|(i, b)| (format!("{label}#{i}"), b.content))
        .collect()
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// `(label, render source)` for every shipped render source, labelled by its
/// path under `assets/` (or the kitchen profile name) and its index there.
pub fn shipped_render_strings() -> Vec<(String, String)> {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let mut out = Vec::new();
    for (label, yaml) in [
        (
            "kitchen/recipe_profile.yaml",
            holon_kitchen::RECIPE_PROFILE_YAML,
        ),
        (
            "kitchen/shopping_item_profile.yaml",
            holon_kitchen::SHOPPING_ITEM_PROFILE_YAML,
        ),
    ] {
        out.extend(yaml_renders(label, yaml));
    }
    let mut files = Vec::new();
    files_under(&assets.join("default"), &mut files);
    files_under(&assets.join("integrations"), &mut files);
    files.sort();
    for path in files {
        let label = path
            .strip_prefix(&assets)
            .expect("under assets")
            .to_string_lossy()
            .to_string();
        match path.extension().and_then(|e| e.to_str()) {
            Some("yaml") => {
                let text = std::fs::read_to_string(&path).expect("asset readable");
                out.extend(yaml_renders(&label, &text));
            }
            Some("org") => {
                let text = std::fs::read_to_string(&path).expect("asset readable");
                out.extend(org_renders(&label, &text));
            }
            _ => {}
        }
    }
    out
}
