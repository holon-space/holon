//! Increment 2.5 (#45 design §6.4): the smallest real experiment de-risking the
//! Petri-Net route's dominant cost — **catalog truthfulness**.
//!
//! The claim under test: a transition's *declared* arcs (what it reads, what it
//! writes) can be stated up front and stay true to what the provider actually
//! does. If they can, a declared firing may be held, simulated, scored or fired
//! by policy (§6.2 Q1/Q5). If they silently diverge, the catalog is
//! "misinformation with authority" (§6.2 Q3-i).
//!
//! Method — ONE arrangement, TWO transitions:
//!   1. Declare `daily_journal_emit` (the journal rule, the one transition
//!      already running through the PN) and `instantiate_template_tpl3` (a
//!      compound) as holon-engine transitions, in the engine's own YAML arc
//!      language (`NET_YAML` below — this IS the catalog entry).
//!   2. Materialize the simulated marking from the REAL pre-op SQL state.
//!   3. Fire BOTH sides: `Engine::fire` on the marking, and the real operation
//!      through a booted `TestEnvironment` (production DI graph).
//!   4. Diff the simulated marking against the real post-op SQL state.
//!
//! The diff is the oracle. Empty ⇒ the declaration is truthful. Non-empty ⇒
//! either the declaration was wrong (corrected here, that is the learning) or
//! the provider does undeclared work (pinned below as a constant, so a future
//! drift reds this test instead of quietly widening the lie).
//!
//! This is an EXPERIMENT, not a feature: no production code is touched, and
//! every inexpressibility is recorded in `EXPRESSIVENESS_GAPS` rather than
//! worked around by extending the engine.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_api::Value;
use holon_api::link_parser::PageId;
use holon_engine::engine::Engine as PnEngine;
use holon_engine::value::Value as PnValue;
use holon_engine::yaml::net::YamlNet;
use holon_engine::yaml::state::YamlMarking;
use holon_engine::yaml::state::YamlToken;
use holon_integration_tests::test_environment::TestEnvironmentBuilder;

/// The catalog under test. Two transitions, declared in holon-engine's arc
/// language exactly as a PN-route catalog entry would be.
///
/// Read the `precond`s as input arcs, the `postcond`s as effect arcs on
/// existing tokens, and `creates` as effect arcs minting new tokens. Every
/// deviation from the natural declaration is annotated with the engine
/// limitation that forced it (see `EXPRESSIVENESS_GAPS`).
const NET_YAML: &str = r#"
transitions:
  # The journal rule: `when: not block_exists("Journals/{today}")`,
  # `emit: {place: page(journals), name: "{today}"}`.
  daily_journal_emit:
    inputs:
      # GAP-7: a captured placeholder ("$today") is keyed WITH its `$`, so it
      # is only usable as a whole-expression copy — never inside an
      # expression. The clock is therefore bound with no precond and read as
      # `clock.today`.
      - bind: clock
        token_type: clock
      - bind: journals
        token_type: block
        precond:
          id: "block:journals"
          # GAP-1: the rule's `not block_exists(...)` is an INHIBITOR arc.
          # Flat nets have no inhibitor arcs, so the inhibitor is encoded as a
          # status attribute the HARNESS must maintain (production computes it
          # with a `block_raw` read the net cannot express).
          day_page_present: "false"
    outputs:
      # GAP-2: a pure READ arc is not expressible — `validate()` requires every
      # input bind to be consumed or re-produced, so a read shows up as an
      # identity output arc.
      - from: clock
      - from: journals
        postcond:
          day_page_present: '"true"'
    creates:
      # GAP-3: the real id is `PageId::for_path(<page-path>/<today>)`, a UUIDv5.
      # It IS a pure function of declared inputs, but the arc expression
      # language has no uuid5 builtin, so the declaration mints the *path* and
      # the test normalizes it through the production minting function.
      # GAP-4: `journals.page_path` is a transitive-closure read up the parent
      # relation. A flat net has attributes but no relations, so the path must
      # be supplied to the marking from outside the net.
      - id_expr: 'journals.page_path + "/" + clock.today'
        token_type: block
        attrs:
          parent_id: 'journals.id'
          content: 'clock.today'
          is_page: '1'
    duration: 0

  # The compound: `instantiate_template` over a THREE-node template.
  # GAP-5: the real operation's effect arity is DATA-dependent (one create per
  # template node). A transition has a static arc set, so this entry can only
  # declare one fixed template shape — the catalog cannot hold a generic
  # `instantiate_template`.
  instantiate_template_tpl3:
    inputs:
      # GAP-7: a captured placeholder ("$today") is keyed WITH its `$`, so it
      # is only usable as a whole-expression copy — never inside an
      # expression. The clock is therefore bound with no precond and read as
      # `clock.today`.
      - bind: clock
        token_type: clock
      - bind: target
        token_type: block
        precond:
          id: "block:pn-target"
      - bind: tpl_root
        token_type: block
        precond:
          id: "block:pn-tpl-root"
      - bind: tpl_a
        token_type: block
        precond:
          id: "block:pn-tpl-a"
      - bind: tpl_b
        token_type: block
        precond:
          id: "block:pn-tpl-b"
    outputs:
      - from: clock
      - from: target
      - from: tpl_root
      - from: tpl_a
      - from: tpl_b
    creates:
      # GAP-3 again: real ids are `deterministic_instance_id(template_id,
      # context_key, node_id)` (UUIDv5); declared as the id TUPLE and
      # normalized test-side.
      # GAP-6: `{{var}}` substitution is not expressible — Rhai's `replace`
      # mutates in place and the arc language compiles a single EXPRESSION, so
      # the declaration must embed the template's literal text and state the
      # substituted RESULT.
      - id_expr: '"inst" + tpl_root.id'
        token_type: block
        attrs:
          parent_id: 'target.id'
          content: '"Daily " + clock.today'
      - id_expr: '"inst" + tpl_a.id'
        token_type: block
        attrs:
          parent_id: '"inst" + tpl_root.id'
          content: '"Morning " + clock.today'
      - id_expr: '"inst" + tpl_b.id'
        token_type: block
        attrs:
          parent_id: '"inst" + tpl_root.id'
          content: '"Evening"'
    duration: 0

objective:
  expr: "0.0"
"#;

/// What the arc language could not say, found by writing the two entries above.
/// Reported, never worked around.
const EXPRESSIVENESS_GAPS: &[&str] = &[
    "GAP-1 inhibitor arc (`not block_exists`) — not expressible; encoded as a harness-maintained \
     status attribute",
    "GAP-2 read-only arc — not expressible; every input must be consumed or re-produced",
    "GAP-3 derived identity (UUIDv5 of a name-tuple) — pure function of declared inputs, but not \
     expressible in the arc expression language",
    "GAP-4 ancestor-chain read (page path) — flat nets carry attributes, not relations",
    "GAP-5 data-dependent effect arity (one create per template node) — a transition's arc set is \
     static",
    "GAP-6 `{{var}}` substitution — not expressible; the declaration must embed the template's \
     literal text and state the substituted result",
    "GAP-7 a captured placeholder is keyed with its `$`, so it is not a usable Rhai variable — a \
     bound value can only be copied verbatim, never combined inside an expression",
];

/// Columns of a created `block_raw` row that the declaration does NOT and
/// cannot state, i.e. the provider's undeclared work. Pinned: if the provider
/// starts writing MORE than this, the test reds.
const UNDECLARED_CREATED_COLUMNS: &[&str] = &[
    "sort_key",       // minted by the consolidator (Model.md invariant 2)
    "created_at",     // wall-clock stamp
    "updated_at",     // wall-clock stamp
    "_change_origin", // provenance stamp
];

// ---------------------------------------------------------------------------
// SQL-side state capture
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
struct BlockRow {
    parent_id: String,
    content: String,
    sort_key: String,
    content_type: String,
    block_type: String,
    properties: String,
    is_page: bool,
}

type BlockRows = BTreeMap<String, BlockRow>;

async fn snapshot_blocks(
    env: &holon_integration_tests::test_environment::TestEnvironment,
) -> BlockRows {
    let rows = env
        .query_sql(
            "SELECT b.id AS id, b.parent_id AS parent_id, b.content AS content, b.sort_key AS \
             sort_key, b.content_type AS content_type, b.block_type AS block_type, \
             COALESCE(b.properties, '') AS properties, \
             CASE WHEN t.block_id IS NULL THEN 0 ELSE 1 END AS is_page \
             FROM block_raw b LEFT JOIN block_tags t ON t.block_id = b.id AND t.tag = 'Page'",
        )
        .await
        .expect("block_raw snapshot");
    let text = |row: &holon_api::widget_spec::DataRow, col: &str| -> String {
        row.get(col)
            .and_then(|v| v.as_string())
            .unwrap_or_default()
            .to_string()
    };
    rows.iter()
        .map(|row| {
            let id = text(row, "id");
            let is_page = match row.get("is_page") {
                Some(Value::Integer(i)) => *i != 0,
                Some(Value::String(s)) => s.as_str() != "0",
                other => panic!("is_page: unexpected {other:?}"),
            };
            (
                id,
                BlockRow {
                    parent_id: text(row, "parent_id"),
                    content: text(row, "content"),
                    sort_key: text(row, "sort_key"),
                    content_type: text(row, "content_type"),
                    block_type: text(row, "block_type"),
                    properties: text(row, "properties"),
                    is_page,
                },
            )
        })
        .collect()
}

/// Rows present in `post` and absent from `pre`.
fn created_rows(pre: &BlockRows, post: &BlockRows) -> BlockRows {
    post.iter()
        .filter(|(id, _)| !pre.contains_key(*id))
        .map(|(id, row)| (id.clone(), row.clone()))
        .collect()
}

/// Ids whose modelled columns changed between the two snapshots.
fn changed_ids(pre: &BlockRows, post: &BlockRows) -> BTreeSet<String> {
    post.iter()
        .filter(|(id, row)| pre.get(*id).is_some_and(|old| old != *row))
        .map(|(id, _)| id.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Marking materialization: the real SQL state, forked into a PN marking
// ---------------------------------------------------------------------------

fn s(v: &str) -> PnValue {
    PnValue::String(v.to_string())
}

/// Build the simulated marking from real rows, plus the two attributes the net
/// cannot derive itself (GAP-1 `day_page_present`, GAP-4 `page_path`).
fn marking_from_rows(
    rows: &BlockRows,
    today: &str,
    journals_page_path: &str,
    clock_now: chrono::DateTime<chrono::Utc>,
) -> YamlMarking {
    let day_page_present = rows
        .values()
        .any(|r| r.parent_id == "block:journals" && r.content == today);
    let mut tokens: Vec<YamlToken> = rows
        .iter()
        .map(|(id, row)| {
            let mut attributes = BTreeMap::new();
            attributes.insert("id".to_string(), s(id));
            attributes.insert("parent_id".to_string(), s(&row.parent_id));
            attributes.insert("content".to_string(), s(&row.content));
            attributes.insert("is_page".to_string(), PnValue::Int(row.is_page as i64));
            if id == "block:journals" {
                attributes.insert("page_path".to_string(), s(journals_page_path));
                attributes.insert(
                    "day_page_present".to_string(),
                    s(if day_page_present { "true" } else { "false" }),
                );
            }
            YamlToken {
                name: id.clone(),
                token_type: "block".to_string(),
                attributes,
            }
        })
        .collect();
    let mut clock_attrs = BTreeMap::new();
    clock_attrs.insert("today".to_string(), s(today));
    tokens.push(YamlToken {
        name: "clock".to_string(),
        token_type: "clock".to_string(),
        attributes: clock_attrs,
    });
    YamlMarking {
        clock: clock_now,
        tokens,
    }
}

/// Tokens present in `after` and absent from `before`, as (id, attrs).
fn created_tokens(
    before: &YamlMarking,
    after: &YamlMarking,
) -> BTreeMap<String, BTreeMap<String, PnValue>> {
    let existing: BTreeSet<&str> = before.tokens.iter().map(|t| t.name.as_str()).collect();
    after
        .tokens
        .iter()
        .filter(|t| !existing.contains(t.name.as_str()))
        .map(|t| (t.name.clone(), t.attributes.clone()))
        .collect()
}

fn attr(attrs: &BTreeMap<String, PnValue>, key: &str) -> String {
    match attrs.get(key) {
        Some(PnValue::String(v)) => v.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// The experiment
// ---------------------------------------------------------------------------

#[test]
fn declared_transition_arcs_are_diffed_against_the_real_operations() {
    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .expect("tokio runtime"),
    );
    rt.clone().block_on(async move {
        let mut findings: Vec<String> = Vec::new();

        // --- boot with a pinned clock, so `today` is deterministic ----------
        let day1 = chrono::NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
        let pinned_ms = day1.and_hms_opt(9, 0, 0).unwrap().and_utc().timestamp_millis();
        let clock = Arc::new(holon_api::TestClock::new(pinned_ms));
        let env = TestEnvironmentBuilder::new()
            .with_clock(clock.clone())
            .build(rt.clone())
            .await
            .expect("boot the production DI graph");
        env.wait_for_cdc_quiescent(Duration::from_millis(400), Duration::from_secs(20))
            .await;

        let net_path = env.temp_path().join("pn_catalog.yaml");
        std::fs::write(&net_path, NET_YAML).expect("write the catalog");
        let net = YamlNet::load(&net_path).expect("the catalog must load");
        let structural = net.validate();
        assert!(
            structural.is_empty(),
            "the declared catalog is structurally invalid: {structural:?}"
        );
        let pn = PnEngine::new();

        // The boot firing already minted day1's page. Use it to recover the
        // journals page path (GAP-4): the harness cannot ask the net for it.
        let day1_str = day1.format("%Y-%m-%d").to_string();
        let boot_rows = snapshot_blocks(&env).await;
        let boot_day_id = boot_rows
            .iter()
            .find(|(_, r)| r.parent_id == "block:journals" && r.content == day1_str)
            .map(|(id, _)| id.clone())
            .expect("the boot journal firing must have minted day1's page");
        let journals_page_path = ["Journals", "journals", "Journal"]
            .into_iter()
            .find(|p| {
                PageId::for_path(&format!("{p}/{day1_str}"))
                    .map(|pid| pid.as_str() == boot_day_id)
                    .unwrap_or(false)
            })
            .unwrap_or_else(|| {
                panic!(
                    "could not recover the journals page path from the boot-minted id \
                     {boot_day_id} — the harness must supply it (GAP-4)"
                )
            })
            .to_string();
        findings.push(format!(
            "journals page path recovered empirically: {journals_page_path:?} (GAP-4: not \
             derivable inside the net)"
        ));

        // ================================================================
        // TRANSITION 1 — daily_journal_emit
        // ================================================================
        let day2 = day1.succ_opt().unwrap();
        let day2_str = day2.format("%Y-%m-%d").to_string();

        let pre = snapshot_blocks(&env).await;
        // The clock rollover is an ENVIRONMENT move (§6.1: the marking changes
        // with no local firing), so it is applied to BOTH sides before firing.
        let sim_before = marking_from_rows(&pre, &day2_str, &journals_page_path, chrono::Utc::now());

        // -- real firing: advance the injected clock, run the production
        //    reconciler, let the rule watcher fire.
        clock.advance(86_400_000);
        holon::sync::clock_scheduler::reconcile_clock(env.engine().db_handle(), clock.as_ref())
            .await
            .expect("reconcile_clock");
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let now = snapshot_blocks(&env).await;
            if now
                .values()
                .any(|r| r.parent_id == "block:journals" && r.content == day2_str)
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the journal rule never fired for {day2_str}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        env.wait_for_cdc_quiescent(Duration::from_millis(400), Duration::from_secs(20))
            .await;
        let post = snapshot_blocks(&env).await;

        // -- simulated firing
        let mut sim_after = sim_before.clone();
        let enabled = pn.enabled(&net, &sim_after).expect("enabled");
        let binding = enabled
            .iter()
            .find(|b| b.transition_id == "daily_journal_emit")
            .unwrap_or_else(|| {
                panic!(
                    "daily_journal_emit is NOT enabled on the real pre-state — the declared input \
                     arcs are already untruthful. enabled={:?}",
                    enabled.iter().map(|b| &b.transition_id).collect::<Vec<_>>()
                )
            })
            .clone();
        pn.fire(&net, &mut sim_after, &binding, 1).expect("fire");

        let journal_verdict = diff_verdict(
            "daily_journal_emit",
            &pre,
            &post,
            &sim_before,
            &sim_after,
            &|declared_id: &str| {
                PageId::for_path(declared_id)
                    .expect("declared path id must normalize")
                    .as_str()
                    .to_string()
            },
            &mut findings,
        );

        // ================================================================
        // TRANSITION 2 — instantiate_template over a 3-node template
        // ================================================================
        let create = |id: &str, parent: &str, content: &str, props: Option<&str>| {
            let mut p: HashMap<String, Value> = HashMap::new();
            p.insert("id".into(), Value::String(id.into()));
            p.insert("parent_id".into(), Value::String(parent.into()));
            p.insert("content".into(), Value::String(content.into()));
            if let Some(props) = props {
                p.insert("properties".into(), Value::String(props.into()));
            }
            p
        };
        for params in [
            create("block:pn-target", "block:journals", "PN instantiation target", None),
            create(
                "block:pn-tpl-root",
                "block:journals",
                "Daily {{date}}",
                Some(r#"{"template":"true","template_vars":"date"}"#),
            ),
            create("block:pn-tpl-a", "block:pn-tpl-root", "Morning {{date}}", None),
            create("block:pn-tpl-b", "block:pn-tpl-root", "Evening", None),
        ] {
            env.execute_operation("block", "create", params)
                .await
                .expect("seed the template subtree");
        }
        env.wait_for_cdc_quiescent(Duration::from_millis(400), Duration::from_secs(20))
            .await;

        let pre2 = snapshot_blocks(&env).await;
        let sim2_before =
            marking_from_rows(&pre2, &day2_str, &journals_page_path, chrono::Utc::now());

        const CONTEXT_KEY: &str = "pn-probe-key";
        let mut params: HashMap<String, Value> = HashMap::new();
        params.insert("template_id".into(), Value::String("block:pn-tpl-root".into()));
        params.insert("target_parent".into(), Value::String("block:pn-target".into()));
        params.insert("context_key".into(), Value::String(CONTEXT_KEY.into()));
        let mut bindings: HashMap<String, Value> = HashMap::new();
        bindings.insert("date".into(), Value::String(day2_str.clone()));
        params.insert("bindings".into(), Value::Object(bindings));
        env.execute_operation("block", "instantiate_template", params)
            .await
            .expect("instantiate_template dispatch");
        env.wait_for_cdc_quiescent(Duration::from_millis(400), Duration::from_secs(20))
            .await;
        let post2 = snapshot_blocks(&env).await;

        let mut sim2_after = sim2_before.clone();
        let enabled2 = pn.enabled(&net, &sim2_after).expect("enabled");
        let binding2 = enabled2
            .iter()
            .find(|b| b.transition_id == "instantiate_template_tpl3")
            .unwrap_or_else(|| {
                panic!(
                    "instantiate_template_tpl3 is NOT enabled on the real pre-state — declared \
                     input arcs untruthful. enabled={:?}",
                    enabled2.iter().map(|b| &b.transition_id).collect::<Vec<_>>()
                )
            })
            .clone();
        pn.fire(&net, &mut sim2_after, &binding2, 2).expect("fire");

        let template_verdict = diff_verdict(
            "instantiate_template_tpl3",
            &pre2,
            &post2,
            &sim2_before,
            &sim2_after,
            &|declared_id: &str| {
                // "inst<template-node-id>" → the production mint.
                let node = declared_id
                    .strip_prefix("inst")
                    .expect("declared instance id shape");
                holon_api::effect_id::deterministic_instance_id(
                    "block:pn-tpl-root",
                    CONTEXT_KEY,
                    node,
                )
                .as_str()
                .to_string()
            },
            &mut findings,
        );

        // ================================================================
        // Report + oracle
        // ================================================================
        println!("\n=== PN CATALOG TRUTHFULNESS PROBE ===");
        for gap in EXPRESSIVENESS_GAPS {
            println!("  [expressiveness] {gap}");
        }
        for f in &findings {
            println!("  [finding] {f}");
        }
        println!("{journal_verdict}");
        println!("{template_verdict}");

        assert!(
            journal_verdict.declared_creates_matched,
            "daily_journal_emit: the declared create arc did not match the real create\n{journal_verdict}"
        );
        assert!(
            template_verdict.declared_creates_matched,
            "instantiate_template_tpl3: the declared create arcs did not match the real creates\n{template_verdict}"
        );
        // The undeclared columns are the pinned residue. A widening reds here.
        for verdict in [&journal_verdict, &template_verdict] {
            for col in &verdict.undeclared_columns {
                assert!(
                    UNDECLARED_CREATED_COLUMNS.contains(&col.as_str()),
                    "{}: the provider wrote an UNDECLARED column '{col}' that is not in the pinned \
                     residue {UNDECLARED_CREATED_COLUMNS:?}\n{verdict}",
                    verdict.transition
                );
            }
        }
    });
}

// ---------------------------------------------------------------------------
// The diff oracle
// ---------------------------------------------------------------------------

struct Verdict {
    transition: String,
    declared_creates_matched: bool,
    lines: Vec<String>,
    undeclared_columns: Vec<String>,
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "  --- {} ---", self.transition)?;
        for line in &self.lines {
            writeln!(f, "    {line}")?;
        }
        Ok(())
    }
}

/// Diff the simulated marking delta against the real SQL delta.
fn diff_verdict(
    transition: &str,
    pre: &BlockRows,
    post: &BlockRows,
    sim_before: &YamlMarking,
    sim_after: &YamlMarking,
    normalize_id: &dyn Fn(&str) -> String,
    findings: &mut Vec<String>,
) -> Verdict {
    let real_created = created_rows(pre, post);
    let sim_created = created_tokens(sim_before, sim_after);
    let mut lines = Vec::new();
    let mut undeclared_columns: Vec<String> = Vec::new();
    let mut matched = true;

    lines.push(format!(
        "real created {} row(s), simulation created {} token(s)",
        real_created.len(),
        sim_created.len()
    ));

    let normalized: BTreeMap<String, BTreeMap<String, PnValue>> = sim_created
        .iter()
        .map(|(id, attrs)| (normalize_id(id), attrs.clone()))
        .collect();

    for (sim_id, attrs) in &normalized {
        match real_created.get(sim_id) {
            None => {
                matched = false;
                lines.push(format!(
                    "PHANTOM: the declaration predicts a block {sim_id} the operation never created"
                ));
            }
            Some(row) => {
                // A declared parent that names one of THIS firing's own created
                // tokens carries the declared (un-minted) id, so it goes
                // through the same normalization (GAP-3).
                let declared_parent_raw = attr(attrs, "parent_id");
                let declared_parent = if sim_created.contains_key(&declared_parent_raw) {
                    normalize_id(&declared_parent_raw)
                } else {
                    declared_parent_raw
                };
                let declared_content = attr(attrs, "content");
                if declared_parent != row.parent_id {
                    matched = false;
                    lines.push(format!(
                        "{sim_id}: declared parent_id {declared_parent:?} but real {:?}",
                        row.parent_id
                    ));
                }
                if declared_content != row.content {
                    matched = false;
                    lines.push(format!(
                        "{sim_id}: declared content {declared_content:?} but real {:?}",
                        row.content
                    ));
                }
                if declared_parent == row.parent_id && declared_content == row.content {
                    lines.push(format!("MATCH: {sim_id} (parent_id, content) as declared"));
                }
                // Non-default columns the declaration never mentions.
                for (col, value, default) in [
                    ("sort_key", row.sort_key.as_str(), "A0"),
                    ("content_type", row.content_type.as_str(), "text"),
                    ("block_type", row.block_type.as_str(), "text"),
                    ("properties", row.properties.as_str(), ""),
                ] {
                    if value != default && !undeclared_columns.contains(&col.to_string()) {
                        undeclared_columns.push(col.to_string());
                    }
                }
                if !undeclared_columns.contains(&"sort_key".to_string()) {
                    // sort_key is always minted, even when it equals the default.
                    undeclared_columns.push("sort_key".to_string());
                }
                if row.is_page && attr(attrs, "is_page") != "1" {
                    matched = false;
                    lines.push(format!(
                        "{sim_id}: real row is Page-tagged, the declaration does not say so"
                    ));
                }
                if !row.is_page && attr(attrs, "is_page") == "1" {
                    matched = false;
                    lines.push(format!(
                        "{sim_id}: declaration claims a Page tag the real row does not carry"
                    ));
                }
            }
        }
    }

    for real_id in real_created.keys() {
        if !normalized.contains_key(real_id) {
            matched = false;
            lines.push(format!(
                "UNDECLARED WORK: the operation created {real_id} \
                 (parent={:?}, content={:?}) that no declared arc predicts",
                real_created[real_id].parent_id, real_created[real_id].content
            ));
        }
    }

    let real_changed = changed_ids(pre, post);
    let sim_changed: BTreeSet<String> = sim_after
        .tokens
        .iter()
        .filter(|t| {
            sim_before
                .tokens
                .iter()
                .any(|old| old.name == t.name && old.attributes != t.attributes)
        })
        .map(|t| t.name.clone())
        .collect();
    // `day_page_present` is a harness-maintained shadow attribute (GAP-1), not
    // a SQL column, so a simulated change to it has no real counterpart by
    // construction. Report it rather than hide it.
    for id in &sim_changed {
        if !real_changed.contains(id) {
            findings.push(format!(
                "{transition}: the declaration mutates {id} (status attribute) with no \
                 corresponding SQL column — GAP-1 shadow state"
            ));
        }
    }
    for id in &real_changed {
        if !sim_changed.contains(id) {
            lines.push(format!(
                "UNDECLARED WORK: the operation modified the existing block {id} \
                 (no declared effect arc)"
            ));
            matched = false;
        }
    }
    if !undeclared_columns.is_empty() {
        lines.push(format!(
            "undeclared-but-written columns on created rows: {undeclared_columns:?}"
        ));
    }

    Verdict {
        transition: transition.to_string(),
        declared_creates_matched: matched,
        lines,
        undeclared_columns,
    }
}
