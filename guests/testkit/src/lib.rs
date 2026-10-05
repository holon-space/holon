//! A deliberately misbehaving guest, for the host's failure-mode suite.
//!
//! One `.wasm` rather than one per behaviour: the behaviour is chosen by the
//! FILE CONTENT, which is the only channel a `FileFormatAdapter` gives a test.
//! The first line of the file names it; anything else is the well-formed
//! stream.
//!
//! Every behaviour here is a way a third-party plugin can be wrong, and the
//! host must answer each with a named `Err` rather than with silence.

use holon_plugin_rows::DocumentRow;
use holon_plugin_rows::Fields;
use holon_plugin_rows::Line;
use holon_plugin_rows::LocalId;
use holon_plugin_rows::Owner;
use holon_plugin_rows::Row;
use holon_plugin_rows::Scope;
use holon_plugin_rows::Stream;
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
    let document = || document_titled(path, Fields::new());
    let thing = || row("thing", path, &[("source_path", json!(path))]);
    let scopes = || vec![scope("thing", "source_path", path)];

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
            vec![scope("mystery", "source_path", path)],
            vec![
                document(),
                row("mystery", path, &[("source_path", json!(path))]),
            ],
        )),
        "undeclared_column" => Ok(render(
            scopes(),
            vec![
                document(),
                row(
                    "thing",
                    path,
                    &[("source_path", json!(path)), ("surprise", json!(1))],
                ),
            ],
        )),
        "wrong_owner_column" => Ok(render(
            vec![scope("thing", "id", path)],
            vec![document(), row("thing", path, &[])],
        )),
        // The typed id cannot spell these, so they are written as raw lines:
        // what a guest not built on `holon-plugin-rows` could still emit.
        "malformed_id" => Ok(raw(
            scopes(),
            document(),
            json!({"row": {"type": "thing", "id": {"path": [path, ""]}, "cells": {"source_path": path}}}),
        )),
        "joined_id" => Ok(raw(
            scopes(),
            document(),
            json!({"row": {"type": "thing", "id": "already:schemed", "cells": {"source_path": path}}}),
        )),
        "foreign_id" => Ok(render(
            scopes(),
            vec![
                document(),
                row("thing", "some/other/file", &[("source_path", json!(path))]),
            ],
        )),
        "row_outside_its_scope" => Ok(render(
            scopes(),
            vec![
                document(),
                row("thing", path, &[("source_path", json!("some/other/file"))]),
            ],
        )),
        "no_document" => Ok(render(scopes(), vec![thing()])),
        "two_documents" => Ok(render(
            scopes(),
            vec![
                document(),
                document_titled("a second document", Fields::new()),
                thing(),
            ],
        )),
        "storage_column_property" => {
            let mut properties = Fields::new();
            properties.insert("content", json!("overwrites the row's own text"));
            Ok(render(
                scopes(),
                vec![document_titled(path, properties), thing()],
            ))
        }
        "missing_scope" => Ok(render(Vec::new(), vec![document()])),
        "empty_scope" => Ok(render(scopes(), vec![document()])),
        _ => Ok(render(scopes(), vec![document(), thing()])),
    }
}

fn scope(type_name: &str, owner_column: &str, owner: &str) -> Scope {
    Scope {
        type_name: type_name.to_string(),
        owner_column: owner_column.to_string(),
        owner: Owner::Text(owner.to_string()),
    }
}

fn document_titled(title: &str, properties: Fields<Value>) -> Line {
    Line::Document(DocumentRow {
        title: title.to_string(),
        properties,
    })
}

fn row(type_name: &str, path: &str, cells: &[(&str, Value)]) -> Line {
    let mut fields = Fields::new();
    for (name, value) in cells {
        fields.insert(*name, value.clone());
    }
    Line::Row(Row {
        type_name: type_name.to_string(),
        id: LocalId::from_path(path).expect("the host hands a non-empty source path"),
        refs: Fields::new(),
        cells: fields,
    })
}

fn render(scopes: Vec<Scope>, lines: Vec<Line>) -> String {
    Stream { scopes, lines }.to_jsonl()
}

fn raw(scopes: Vec<Scope>, document: Line, line: Value) -> String {
    let mut out = render(scopes, vec![document]);
    out.push_str(&line.to_string());
    out.push('\n');
    out
}
