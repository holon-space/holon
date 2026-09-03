//! A deliberately misbehaving guest, for the host's failure-mode suite.
//!
//! One `.wasm` rather than one per behaviour: the behaviour is chosen by the
//! FILE CONTENT, which is the only channel a `FileFormatAdapter` gives a test.
//! The first line of the file names it; anything else is the well-formed
//! stream.
//!
//! Every behaviour here is a way a third-party plugin can be wrong, and the
//! host must answer each with a named `Err` rather than with silence.

use serde_json::Value;
use serde_json::json;

holon_abi_guest::holon_plugin!(misbehave);

fn misbehave(input: &[u8], ctx: &[u8]) -> Result<String, String> {
    let source = core::str::from_utf8(input).map_err(|e| format!("input is not UTF-8: {e}"))?;
    let ctx: Value =
        serde_json::from_slice(ctx).map_err(|e| format!("context is not JSON: {e}"))?;
    let path = ctx["source_path"]
        .as_str()
        .ok_or("context is missing `source_path`")?;
    let behavior = source.lines().next().unwrap_or("").trim();

    // Both declared scopes and both rows: what a well-behaved guest emits, and
    // the baseline each misbehaviour below departs from in exactly one way.
    let document = json!({"type": "holon.document", "row": {"title": path}});
    let thing = json!({"type": "thing", "row": {"id": path, "source_path": path}});
    let scopes = || {
        vec![
            scope("holon.document", "source_path", path),
            scope("thing", "source_path", path),
        ]
    };

    match behavior {
        "refuse" => Err("the testkit guest refuses this file by request".to_string()),
        "trap" => panic!("the testkit guest traps by request"),
        "spin" => {
            let mut n: u64 = 0;
            loop {
                n = n.wrapping_add(1);
                core::hint::black_box(n);
            }
        }
        "devour" => {
            let mut held: Vec<Vec<u8>> = Vec::new();
            loop {
                held.push(vec![0u8; 4 * 1024 * 1024]);
                core::hint::black_box(&held);
            }
        }
        "not_a_stream" => Ok("this is not an envelope\n".to_string()),

        "undeclared_scope" => Ok(render(
            &[
                scope("holon.document", "source_path", path),
                scope("mystery", "source_path", path),
            ],
            &[
                document,
                json!({"type": "mystery", "row": {"id": path, "source_path": path}}),
            ],
        )),
        "undeclared_column" => Ok(render(
            &scopes(),
            &[
                document,
                json!({"type": "thing", "row": {"id": path, "source_path": path, "surprise": 1}}),
            ],
        )),
        "wrong_owner_column" => Ok(render(
            &[
                scope("holon.document", "source_path", path),
                scope("thing", "id", path),
            ],
            &[document, json!({"type": "thing", "row": {"id": path}})],
        )),
        "unstorable_id" => Ok(render(
            &scopes(),
            &[
                document,
                json!({"type": "thing", "row": {"id": "already:schemed", "source_path": path}}),
            ],
        )),
        "row_outside_its_scope" => Ok(render(
            &scopes(),
            &[
                document,
                json!({"type": "thing", "row": {"id": path, "source_path": "some/other/file"}}),
            ],
        )),
        "no_document" => Ok(render(
            &[scope("thing", "source_path", path)],
            &[thing.clone()],
        )),
        "two_documents" => Ok(render(
            &scopes(),
            &[
                document,
                json!({"type": "holon.document", "row": {"title": "a second document"}}),
                thing.clone(),
            ],
        )),
        "storage_column_property" => Ok(render(
            &scopes(),
            &[
                json!({
                    "type": "holon.document",
                    "row": {"title": path, "content": "overwrites the row's own text"}
                }),
                thing.clone(),
            ],
        )),
        "missing_scope" => Ok(render(
            &[scope("holon.document", "source_path", path)],
            &[document],
        )),
        "empty_scope" => Ok(render(&scopes(), &[document])),
        _ => Ok(render(&scopes(), &[document, thing])),
    }
}

fn scope(type_name: &str, owner_column: &str, owner_value: &str) -> Value {
    json!({"type": type_name, "owner_column": owner_column, "owner_value": owner_value})
}

fn render(scopes: &[Value], lines: &[Value]) -> String {
    let mut out = json!({"holon_rows": 1, "scopes": scopes}).to_string();
    out.push('\n');
    for line in lines {
        out.push_str(&line.to_string());
        out.push('\n');
    }
    out
}
