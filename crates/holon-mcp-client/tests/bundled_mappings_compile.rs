//! Every jaq mapping this build ships compiles against the function set a
//! mapping is given.

use holon_mcp_client::BUNDLED_SIDECARS;
use holon_rows::RowMapper;

#[test]
fn every_bundled_mapping_compiles() {
    let mut compiled = Vec::new();
    for sidecar in BUNDLED_SIDECARS {
        let doc: serde_yaml::Value = serde_yaml::from_str(sidecar.yaml)
            .unwrap_or_else(|e| panic!("{} does not parse: {e}", sidecar.source_path));
        let holon = &doc["holon"];
        let mut sources = Vec::new();
        if let Some(tools) = holon["tools"].as_mapping() {
            for (tool, cfg) in tools {
                for what in ["response", "request"] {
                    if let Some(src) = cfg[what].as_str() {
                        sources.push((
                            format!("holon.tools.{}.{what}", tool.as_str().unwrap()),
                            src,
                        ));
                    }
                }
            }
        }
        if let Some(src) = holon["list_sync"]["key"].as_str() {
            sources.push(("holon.list_sync.key".to_string(), src));
        }
        for (field, src) in sources {
            let label = format!("{}: {field}", sidecar.provider);
            RowMapper::compile(label.clone(), src)
                .unwrap_or_else(|e| panic!("{}: {e:#}", sidecar.source_path));
            compiled.push(label);
        }
    }
    assert!(
        compiled.iter().any(|l| l.starts_with("shopping: ")),
        "the shopping sidecar's mappings were not found, so this walk reads the wrong keys; \
         compiled: {compiled:?}"
    );
}
