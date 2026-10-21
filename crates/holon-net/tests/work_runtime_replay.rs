//! Card #363 spike: one day of agent work replayed through the holon-net
//! executor, to find out what the engine cannot yet express.
//!
//! Test only — no product code. The runtime is the EXISTING [`Executor`], the
//! services are the fixture, the clock is the fixture's `at_s` (seconds since
//! 2026-10-15T00:00:00Z). The snapshot the four assertions read is
//! `at_s = 121560` = 2026-10-16T09:46:00Z = 11:46 local.
//!
//! Fixture provenance (machine-readable sources only, no transcript text):
//!   - `turn_end`, `agent_started`: `session_metrics.py load` parquet over the
//!     six subagent transcripts of 2026-10-16.
//!   - `job_start`, `job_end`: `~/.cache/holon-build/holds.jsonl`, every hold
//!     that started on 2026-10-16 before the snapshot.
//!   - `card_stage`: the steward's 11:46 stall table. `.flow/moves.jsonl` has
//!     NO entry before the snapshot, so the move log cannot supply the stages.
//!
//! The net's shape — its places and arcs — is the file
//! `nets/work-runtime-replay.yaml`, loaded through [`net_def`]. The guards and
//! the firings have no YAML form: `bind` is a Rust `fn` and `Body::pure` a
//! Rust `fn`, so [`logic`] hands one of each in per transition.
//!
//! Twins, each breaking ONE modelling rule, selected by
//! `HOLON_WORK_RUNTIME_TWIN`, so that a passing assertion is shown sensitive:
//!   - `no-restall`: the stored stall is never re-derived. Assertion 1 goes red
//!     on a stale `idle_s` (lane-261 reads 787 s instead of 16 029 s), and
//!     assertion 3's decisions flip off the stale fact — lane-261 and
//!     verify-322 turn resume, lane-341 and lane-343 turn fresh.
//!   - `no-stall-inhibitor`: `NoticeStall` loses its inhibitor and fires again
//!     on unchanged evidence. The whole replay dies with `NoQuiescence(1000)`,
//!     which is assertion 4's claim read as a failure.
//!   - `no-slot-release`: `JobEnded` does not consume the slot. Assertion 2
//!     reads cargo 174 instead of 1.
//!   - `no-resume-ceiling`: the policy publishes no 150k context ceiling, so a
//!     hot agent of any size is resumed. The exclusion check of the ceiling
//!     goes red on the injected 151k agent.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use holon_net::exec::Cause;
use holon_net::exec::Content;
use holon_net::exec::Executor;
use holon_net::exec::NetDef;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Output;
use holon_net::exec::Reader;
use holon_net::exec::Subject;
use holon_net::load::Logic;
use holon_net::load::net_def;
use holon_pattern::Value;

const NET_PATH: &str = "nets/work-runtime-replay.yaml";
const NET: &str = include_str!("../nets/work-runtime-replay.yaml");

const P_CLOCK: &str = "Clock";
const P_BUDGET: &str = "Budget";
const P_CARD: &str = "Card";
const P_AGENT: &str = "Agent";
const P_TURN_ENDED: &str = "TurnEnded";
const P_JOB: &str = "Job";
const P_CPU: &str = "Cpu";
const P_WAITING: &str = "Waiting";
const P_STALLED: &str = "Stalled";
const P_PROPOSAL: &str = "Proposal";
const P_WOKEN: &str = "Woken";

const MACHINE: &str = "machine";
/// A build-slot hold whose owning agent is gone, as the #280 A/B driver's was.
const NO_OWNER: &str = "-";
/// 2026-10-16T09:46:00Z = 11:46 local, the instant the four assertions read.
const SNAPSHOT_S: i64 = 121_560;

/// The six stalled agents of the 11:46 table, with what the fixture measures
/// for each: time since its last turn end, time since its first turn, the
/// context of that last turn, and the continuation ruling 363/a3.b asks for at
/// that idle and context — `resume` under 1 h idle below the 150k ceiling,
/// `fresh` from 1 h idle with context over 95k, `cold` otherwise.
const SIX: [(&str, i64, i64, i64, &str); 6] = [
    ("lane-261", 16_029, 38_320, 84_800, "cold"),
    ("lane-278", 3_220, 38_544, 58_132, "resume"),
    ("lane-291", 3_462, 38_658, 96_476, "resume"),
    ("lane-341", 2_946, 52_411, 123_373, "resume"),
    ("lane-343", 3_412, 52_409, 142_856, "resume"),
    ("verify-322", 16_050, 38_693, 71_315, "cold"),
];
/// The four cards `flow next` charged against the 4-slot cargo at 11:46.
const BOARD_CARGO: [&str; 4] = ["lane-261", "lane-278", "lane-341", "lane-343"];

// ---------------------------------------------------------------- net

/// Each twin breaks ONE modelling rule, so an assertion that passes shows red
/// for exactly the reason it claims. Selected by `HOLON_WORK_RUNTIME_TWIN`.
fn twin() -> String {
    std::env::var("HOLON_WORK_RUNTIME_TWIN").unwrap_or_default()
}

/// The replay net, as the file declares it, with the twin's edit applied to
/// the file text.
fn net() -> NetDef {
    match net_def(NET_PATH, &twinned(NET), logic) {
        Ok(net) => net,
        Err(e) => panic!("{e}"),
    }
}

/// The Rust the net file cannot hold, by transition.
fn logic(transition: &str) -> Logic {
    match transition {
        "AgentTurnEnded" => Logic::pure(Arc::new(bind_turn_ended), Arc::new(body_put_agent)),
        "StartJob" => Logic::pure(Arc::new(bind_start_job), Arc::new(body_start_job)),
        "JobEnded" => Logic::pure(Arc::new(bind_job_ended), Arc::new(body_job_ended)),
        "NoticeStall" => Logic::pure(Arc::new(compute_stall), Arc::new(body_put_stalled)),
        "Restall" => Logic::pure(Arc::new(bind_restall), Arc::new(body_restall)),
        "Continue" => Logic::pure(Arc::new(compute_proposal), Arc::new(body_put_proposal)),
        "Reconsider" => Logic::pure(Arc::new(bind_reconsider), Arc::new(body_put_proposal)),
        "DropProposal" => Logic::pure(Arc::new(bind_drop_proposal), Arc::new(body_nothing)),
        "Handoff" => Logic::pure(Arc::new(bind_handoff), Arc::new(body_woken)),
        other => panic!("the net file declares `{other}`, which has no guard and body here"),
    }
}

/// The net file with the selected twin's one arc or transition cut out.
fn twinned(text: &str) -> String {
    match twin().as_str() {
        "no-slot-release" => without(
            text,
            "    - Cpu\n    produces:\n    - Waiting\n",
            "    produces:\n    - Waiting\n",
        ),
        "no-stall-inhibitor" => without(
            text,
            "    inhibits:\n    - Woken\n    - Stalled\n",
            "    inhibits:\n    - Woken\n",
        ),
        "no-restall" => without(
            text,
            "  Restall:\n    trigger: auto\n    priority: 0\n    consumes:\n    - Stalled\n    reads:\n    - Agent\n    produces:\n    - Stalled\n",
            "",
        ),
        _ => text.to_string(),
    }
}

/// `text` with `from` replaced by `to`, asserting the edit hit the net it was
/// written for.
fn without(text: &str, from: &str, to: &str) -> String {
    assert_eq!(
        text.matches(from).count(),
        1,
        "the twin's edit must hit exactly one spot of the net file"
    );
    text.replace(from, to)
}

/// A turn end moves the agent's clock and context. The evidence is consumed,
/// so the transition fires exactly once per turn.
fn bind_turn_ended(r: &mut Reader<'_>, s: &Subject) -> Option<Content> {
    Some(content([
        ("card", Value::String(r.text(P_AGENT, s, "card")?)),
        ("role", Value::String(r.text(P_AGENT, s, "role")?)),
        (
            "started_s",
            Value::Integer(int_at(r, P_AGENT, s, "started_s")?),
        ),
        (
            "last_turn_end_s",
            Value::Integer(int_at(r, P_TURN_ENDED, s, "at_s")?),
        ),
        (
            "ctx_tokens",
            Value::Integer(int_at(r, P_TURN_ENDED, s, "ctx_tokens")?),
        ),
    ]))
}

fn body_put_agent(s: &Subject, c: &Content) -> Result<Vec<Output>, Cause> {
    Ok(vec![Output::new(P_AGENT, s, c.clone())])
}

/// A hold that has not ended takes the slot. Its subject is the job, because
/// the executor has no counted resource tokens: one token per holder is the
/// only way to count holders.
fn bind_start_job(r: &mut Reader<'_>, s: &Subject) -> Option<Content> {
    if r.field(P_JOB, s, "end_s").is_some() {
        return None;
    }
    Some(content([
        ("ws", Value::String(r.text(P_JOB, s, "ws")?)),
        ("owner", Value::String(r.text(P_JOB, s, "owner")?)),
        ("start_s", Value::Integer(int_at(r, P_JOB, s, "start_s")?)),
    ]))
}

fn body_start_job(s: &Subject, c: &Content) -> Result<Vec<Output>, Cause> {
    let mut out = vec![Output::new(P_CPU, s, c.clone())];
    out.extend(waiting(s, c, true));
    Ok(out)
}

fn bind_job_ended(r: &mut Reader<'_>, s: &Subject) -> Option<Content> {
    Some(content([
        ("end_s", Value::Integer(int_at(r, P_JOB, s, "end_s")?)),
        ("exit", Value::Integer(int_at(r, P_JOB, s, "exit")?)),
        ("owner", Value::String(r.text(P_JOB, s, "owner")?)),
    ]))
}

fn body_job_ended(s: &Subject, c: &Content) -> Result<Vec<Output>, Cause> {
    Ok(waiting(s, c, false))
}

/// `Waiting` is a status field, not a token, because a transition can only
/// consume or inhibit a token of its OWN subject: `JobEnded` fires on the job
/// and cannot retract a token held by the owning agent.
fn waiting(job: &Subject, c: &Content, live: bool) -> Vec<Output> {
    let owner = text_of(c, "owner");
    if owner == NO_OWNER {
        return vec![];
    }
    vec![Output::new(
        P_WAITING,
        &Subject::new(&owner),
        content([
            ("job", Value::String(job.as_str().to_string())),
            ("live", Value::Boolean(live)),
        ]),
    )]
}

/// What the runtime knows about an agent's stall right now, or `None` when it
/// is not stalled. Both `NoticeStall` and `Restall` derive from this one
/// function, so the stored fact and the fresh one are comparable.
fn compute_stall(r: &mut Reader<'_>, s: &Subject) -> Option<Content> {
    let now = int_at(r, P_CLOCK, MACHINE, "now_s")?;
    let idle = now - int_at(r, P_AGENT, s, "last_turn_end_s")?;
    if idle < int_at(r, P_BUDGET, MACHINE, "stall_after_s")? {
        return None;
    }
    if r.field(P_WAITING, s, "live") == Some(Value::Boolean(true)) {
        return None; // waiting on a live build job, not stalled
    }
    let card = r.text(P_AGENT, s, "card")?;
    let stage = r.text(P_CARD, &card, "stage")?;
    Some(content([
        ("idle_s", Value::Integer(idle)),
        (
            "age_s",
            Value::Integer(now - int_at(r, P_AGENT, s, "started_s")?),
        ),
        (
            "board_holds_cargo",
            Value::Boolean(stage == "build" || stage == "verify"),
        ),
    ]))
}

fn body_put_stalled(s: &Subject, c: &Content) -> Result<Vec<Output>, Cause> {
    Ok(vec![Output::new(P_STALLED, s, c.clone())])
}

/// Re-derive a stored stall. The engine has no retraction output and no
/// re-derivation on source change, so a stored fact needs its own transition
/// that consumes it and writes the fresh value back — or nothing, when the
/// agent has revived. The guard is "stored differs from fresh", which the
/// firing itself falsifies, so the net stays terminating.
fn bind_restall(r: &mut Reader<'_>, s: &Subject) -> Option<Content> {
    let stored = content([
        ("idle_s", Value::Integer(int_at(r, P_STALLED, s, "idle_s")?)),
        ("age_s", Value::Integer(int_at(r, P_STALLED, s, "age_s")?)),
        (
            "board_holds_cargo",
            r.field(P_STALLED, s, "board_holds_cargo")?,
        ),
    ]);
    match compute_stall(r, s) {
        Some(fresh) if fresh == stored => None,
        Some(fresh) => Some(fresh),
        None => Some(content([("revived", Value::Boolean(true))])),
    }
}

fn body_restall(s: &Subject, c: &Content) -> Result<Vec<Output>, Cause> {
    if c.contains_key("revived") {
        return Ok(vec![]);
    }
    Ok(vec![Output::new(P_STALLED, s, c.clone())])
}

/// The continuation the runtime proposes for a stalled agent, per ruling
/// 363/a3.b: one decision per agent off the idle clock, with no age band.
fn compute_proposal(r: &mut Reader<'_>, s: &Subject) -> Option<Content> {
    let idle = int_at(r, P_STALLED, s, "idle_s")?;
    let age = int_at(r, P_STALLED, s, "age_s")?;
    let ctx = int_at(r, P_AGENT, s, "ctx_tokens")?;
    let hot = int_at(r, P_BUDGET, MACHINE, "hot_idle_s")?;
    let over = int_at(r, P_BUDGET, MACHINE, "fresh_ctx_over")?;
    let ceiling = int_at(r, P_BUDGET, MACHINE, "resume_ctx_under")?;
    let action = if idle < hot && ctx < ceiling {
        "resume"
    } else if idle >= hot && ctx > over {
        "fresh"
    } else {
        "cold"
    };
    Some(content([
        ("action", Value::String(action.to_string())),
        ("idle_s", Value::Integer(idle)),
        ("age_s", Value::Integer(age)),
        ("ctx_tokens", Value::Integer(ctx)),
    ]))
}

fn body_put_proposal(s: &Subject, c: &Content) -> Result<Vec<Output>, Cause> {
    Ok(vec![Output::new(P_PROPOSAL, s, c.clone())])
}

fn bind_reconsider(r: &mut Reader<'_>, s: &Subject) -> Option<Content> {
    let stored = content([
        ("action", Value::String(r.text(P_PROPOSAL, s, "action")?)),
        (
            "idle_s",
            Value::Integer(int_at(r, P_PROPOSAL, s, "idle_s")?),
        ),
        ("age_s", Value::Integer(int_at(r, P_PROPOSAL, s, "age_s")?)),
        (
            "ctx_tokens",
            Value::Integer(int_at(r, P_PROPOSAL, s, "ctx_tokens")?),
        ),
    ]);
    compute_proposal(r, s).filter(|fresh| *fresh != stored)
}

fn bind_drop_proposal(r: &mut Reader<'_>, s: &Subject) -> Option<Content> {
    Some(content([(
        "action",
        Value::String(r.text(P_PROPOSAL, s, "action")?),
    )]))
}

fn body_nothing(_: &Subject, _: &Content) -> Result<Vec<Output>, Cause> {
    Ok(vec![])
}

fn bind_handoff(r: &mut Reader<'_>, s: &Subject) -> Option<Content> {
    Some(content([
        ("action", Value::String(r.text(P_PROPOSAL, s, "action")?)),
        (
            "at_s",
            Value::Integer(int_at(r, P_CLOCK, MACHINE, "now_s")?),
        ),
    ]))
}

fn body_woken(s: &Subject, c: &Content) -> Result<Vec<Output>, Cause> {
    Ok(vec![Output::new(P_WOKEN, s, c.clone())])
}

// ---------------------------------------------------------------- replay

fn fixture_path() -> String {
    std::env::var("HOLON_WORK_RUNTIME_FIXTURE").unwrap_or_else(|_| {
        format!(
            "{}/tests/fixtures/work-runtime-2026-10-16.jsonl",
            env!("CARGO_MANIFEST_DIR")
        )
    })
}

fn events() -> Vec<serde_json::Map<String, serde_json::Value>> {
    let path = fixture_path();
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("fixture {path}: {e}"));
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(
            |l| match serde_json::from_str(l).expect("fixture line is JSON") {
                serde_json::Value::Object(m) => m,
                other => panic!("fixture line is not an object: {other}"),
            },
        )
        .collect()
}

/// Drive every fixture event into the executor and step to quiescence after
/// each, so no evidence token can be overwritten before it is consumed.
fn replay() -> Executor {
    let mut exec = Executor::new(net()).expect("the net loads");
    let mut last = 0;
    for ev in events() {
        let at_s = ev["at_s"].as_i64().expect("at_s");
        assert!(at_s >= last, "the fixture is not time-ordered at {at_s}");
        last = at_s;
        apply(&mut exec, &ev);
        write(
            &mut exec,
            P_CLOCK,
            MACHINE,
            content([("now_s", Value::Integer(at_s))]),
            at_s,
        );
        let reqs = exec
            .step(Duration::from_secs(at_s as u64))
            .unwrap_or_else(|e| panic!("step at {at_s}: {e:?}"));
        assert!(
            reqs.is_empty(),
            "the net is pure, yet it asked for an effect at {at_s}"
        );
    }
    exec
}

/// An outside commit at the fixture clock. `Executor::write` advances the
/// executor to `at_s` and quiesces on the spot, so the replay also asserts
/// here that the pure net asks for no effect.
fn write(exec: &mut Executor, place: &str, subject: &str, content: Content, at_s: i64) {
    let reqs = exec
        .write(place, subject, content, Duration::from_secs(at_s as u64))
        .unwrap_or_else(|e| panic!("write {place}/{subject} at {at_s}: {e:?}"))
        .requests;
    assert!(
        reqs.is_empty(),
        "the net is pure, yet the write to {place}/{subject} at {at_s} asked for an effect"
    );
}

fn apply(exec: &mut Executor, ev: &serde_json::Map<String, serde_json::Value>) {
    let s = |k: &str| ev[k].as_str().expect(k).to_string();
    let i = |k: &str| ev[k].as_i64().expect(k);
    let at = i("at_s");
    match ev["kind"].as_str().expect("kind") {
        "policy" => {
            // The `no-resume-ceiling` twin publishes no ceiling at all, so a
            // hot agent of any context size is proposed for resume.
            let ceiling = match twin().as_str() {
                "no-resume-ceiling" => i64::MAX,
                _ => i("resume_ctx_under"),
            };
            write(
                exec,
                P_BUDGET,
                MACHINE,
                content([
                    ("stall_after_s", Value::Integer(i("stall_after_s"))),
                    ("hot_idle_s", Value::Integer(i("hot_idle_s"))),
                    ("fresh_ctx_over", Value::Integer(i("fresh_ctx_over"))),
                    ("resume_ctx_under", Value::Integer(ceiling)),
                ]),
                at,
            )
        }
        "card_stage" => write(
            exec,
            P_CARD,
            &s("card"),
            content([
                ("stage", Value::String(s("stage"))),
                ("agent", Value::String(s("agent"))),
            ]),
            at,
        ),
        "agent_started" => write(
            exec,
            P_AGENT,
            &s("agent"),
            content([
                ("card", Value::String(s("card"))),
                ("role", Value::String(s("role"))),
                ("started_s", Value::Integer(i("at_s"))),
                ("last_turn_end_s", Value::Integer(i("at_s"))),
                ("ctx_tokens", Value::Integer(0)),
            ]),
            at,
        ),
        "turn_end" => write(
            exec,
            P_TURN_ENDED,
            &s("agent"),
            content([
                ("at_s", Value::Integer(i("at_s"))),
                ("ctx_tokens", Value::Integer(i("ctx_tokens"))),
            ]),
            at,
        ),
        "job_start" => write(
            exec,
            P_JOB,
            &s("job"),
            content([
                ("ws", Value::String(s("ws"))),
                ("owner", Value::String(s("owner"))),
                ("start_s", Value::Integer(i("at_s"))),
            ]),
            at,
        ),
        "job_end" => write(
            exec,
            P_JOB,
            &s("job"),
            content([
                ("ws", Value::String(s("ws"))),
                ("owner", Value::String(s("owner"))),
                ("end_s", Value::Integer(i("at_s"))),
                ("exit", Value::Integer(i("exit"))),
            ]),
            at,
        ),
        "snapshot" => {}
        other => panic!("unknown fixture event kind {other}"),
    }
}

// ---------------------------------------------------------------- helpers

fn content<const N: usize>(fields: [(&str, Value); N]) -> Content {
    fields
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
}

fn int_at(r: &mut Reader<'_>, place: &str, subject: &str, field: &str) -> Option<i64> {
    match r.field(place, subject, field)? {
        Value::Integer(n) => Some(n),
        other => panic!("{place}/{subject}.{field} is not an integer: {other:?}"),
    }
}

fn text_of(c: &Content, k: &str) -> String {
    match c.get(k) {
        Some(Value::String(s)) => s.clone(),
        other => panic!("field {k} is not a string: {other:?}"),
    }
}

fn int_of(c: &Content, k: &str) -> i64 {
    match c.get(k) {
        Some(Value::Integer(n)) => *n,
        other => panic!("field {k} is not an integer: {other:?}"),
    }
}

/// Every place/subject pair holding a token, as a map of its content.
fn marking(exec: &Executor, place: &str) -> BTreeMap<String, Content> {
    exec.tokens()
        .subjects(place)
        .into_iter()
        .map(|s| {
            let c = exec
                .tokens()
                .token(place, &s)
                .expect("subject holds a token");
            (s.as_str().to_string(), c.content.clone())
        })
        .collect()
}

fn firings(exec: &Executor) -> BTreeMap<String, usize> {
    let mut by_name = BTreeMap::new();
    for (id, _) in exec.occurrences() {
        if let OccurrenceId::Minted { transition, .. } = id {
            *by_name.entry(transition.to_string()).or_insert(0) += 1;
        }
    }
    by_name
}

fn kind_counts() -> BTreeMap<String, usize> {
    let mut by_kind = BTreeMap::new();
    for ev in events() {
        *by_kind
            .entry(ev["kind"].as_str().expect("kind").to_string())
            .or_insert(0) += 1;
    }
    by_kind
}

// ---------------------------------------------------------------- assertions

/// Assertion 1: at 11:46 the net marks exactly the six stalled agents of the
/// steward's table, each with its idle duration.
#[test]
fn marks_exactly_the_six_stalled_agents_with_their_idle_durations() {
    let exec = replay();
    let stalled = marking(&exec, P_STALLED);

    let want: BTreeSet<&str> = SIX.iter().map(|(a, ..)| *a).collect();
    let got: BTreeSet<&str> = stalled.keys().map(String::as_str).collect();
    assert_eq!(
        got, want,
        "the stalled set is not the six of the 11:46 table"
    );

    for (agent, idle_s, age_s, ..) in SIX {
        let c = &stalled[agent];
        assert_eq!(int_of(c, "idle_s"), idle_s, "{agent} idle_s");
        assert_eq!(int_of(c, "age_s"), age_s, "{agent} age_s");
        assert_eq!(
            c.get("board_holds_cargo"),
            Some(&Value::Boolean(true)),
            "{agent}: the board charges its card against cargo",
        );
    }
}

/// Assertion 2: the real cargo at 11:46 is 1, not the 4 the board charged.
/// The one hold belongs to the #280 A/B driver, whose lane agent is gone —
/// none of the four cards the board charged holds a slot.
#[test]
fn cargo_is_one_not_four() {
    let exec = replay();
    let cpu = marking(&exec, P_CPU);

    assert_eq!(cpu.len(), 1, "real cargo at the snapshot: {cpu:?}");
    let (job, held) = cpu.iter().next().expect("one hold");
    assert_eq!(
        text_of(held, "ws"),
        "lane-280-ab-base",
        "holder workspace of {job}"
    );
    assert_eq!(
        text_of(held, "owner"),
        NO_OWNER,
        "the hold belongs to no live agent",
    );

    let owners: BTreeSet<String> = cpu.values().map(|c| text_of(c, "owner")).collect();
    for agent in BOARD_CARGO {
        assert!(
            !owners.contains(agent),
            "{agent} is charged against cargo but holds no slot",
        );
    }
    let charged = marking(&exec, P_STALLED)
        .values()
        .filter(|c| c.get("board_holds_cargo") == Some(&Value::Boolean(true)))
        .count();
    assert!(
        charged > cpu.len(),
        "the net must disclose the board-vs-runtime drift: charged {charged}, held {}",
        cpu.len(),
    );
}

/// Assertion 3, as ruling 363/a3.b amends it: one decision per stalled agent
/// off the idle clock — `resume` when idle is under 1 h and context under
/// 150k, `fresh` when idle is 1 h or more with context over 95k, `cold`
/// otherwise. The expected decision of every agent is the fifth column of
/// [`SIX`], written before the run; no age band takes part.
#[test]
fn proposes_the_per_agent_decision_of_ruling_363_a3b() {
    let exec = replay();
    let proposals = marking(&exec, P_PROPOSAL);

    let want: BTreeSet<&str> = SIX.iter().map(|(a, ..)| *a).collect();
    let got: BTreeSet<&str> = proposals.keys().map(String::as_str).collect();
    assert_eq!(got, want, "one proposal per stalled agent");

    for (agent, idle_s, age_s, ctx, decision) in SIX {
        assert_eq!(
            text_of(&proposals[agent], "action"),
            decision,
            "{agent} idle {idle_s}s age {age_s}s ctx {ctx}: the decision of ruling 363/a3.b"
        );
    }
}

/// The 150k resume ceiling of ruling 363/a3.b, shown to exclude: an agent idle
/// well under 1 h but above the ceiling is NOT resumed. No agent of the
/// recorded day is above it, so this one instance is written here instead, and
/// the `no-resume-ceiling` twin — which publishes no ceiling — turns it into
/// `resume` and sends this red.
#[test]
fn the_resume_ceiling_excludes_a_hot_agent_over_150k() {
    let mut exec = replay();
    write(
        &mut exec,
        P_AGENT,
        "lane-999",
        content([
            ("card", Value::String("261".to_string())),
            ("role", Value::String("build".to_string())),
            ("started_s", Value::Integer(SNAPSHOT_S - 7_200)),
            ("last_turn_end_s", Value::Integer(SNAPSHOT_S - 1_800)),
            ("ctx_tokens", Value::Integer(151_000)),
        ]),
        SNAPSHOT_S,
    );
    write(
        &mut exec,
        P_CLOCK,
        MACHINE,
        content([("now_s", Value::Integer(SNAPSHOT_S))]),
        SNAPSHOT_S,
    );
    exec.step(Duration::from_secs(SNAPSHOT_S as u64))
        .expect("a step on the injected agent");

    assert_eq!(
        text_of(&marking(&exec, P_PROPOSAL)["lane-999"], "action"),
        "cold",
        "idle 1800s with ctx 151000 is over the 150k resume ceiling",
    );
}

/// Assertion 4: no auto transition fires twice on the same evidence. The three
/// evidence-consuming transitions fire exactly once per event, and a second
/// step at the same clock adds no firing at all.
#[test]
fn every_auto_transition_fires_at_most_once_per_evidence_token() {
    let mut exec = replay();
    let fired = firings(&exec);
    let events = kind_counts();

    for (transition, kind) in [
        ("AgentTurnEnded", "turn_end"),
        ("StartJob", "job_start"),
        ("JobEnded", "job_end"),
    ] {
        assert_eq!(
            fired.get(transition).copied().unwrap_or(0),
            events[kind],
            "{transition} must fire once per {kind} evidence token",
        );
    }

    let before = exec.occurrences().count();
    exec.step(Duration::from_secs(SNAPSHOT_S as u64))
        .expect("a second step");
    assert_eq!(
        exec.occurrences().count(),
        before,
        "quiescence is not stable: a step on unchanged evidence fired again",
    );
}

/// VERIFIER PROBE (not part of the lane): assertion 3 as ruling 363/a3.b
/// amends it. Under the ruled idle clock (time since the agent's last turn
/// ended) the check is per agent, not a band: resume when idle is under 1 h
/// and ctx under 150k; fresh when idle is 1 h or more with ctx over 95k;
/// cold otherwise. Expected row per agent, from the replay fixture:
///   lane-261  idle 16029s ctx  84800 -> cold
///   lane-278  idle  3220s ctx  58132 -> resume
///   lane-291  idle  3462s ctx  96476 -> resume
///   lane-341  idle  2946s ctx 123373 -> resume
///   lane-343  idle  3412s ctx 142856 -> resume
///   verify-322 idle 16050s ctx 71315 -> cold
#[test]
fn verifier_probe_a3b_per_agent_decision_under_ruled_idle_clock() {
    let exec = replay();
    let proposals = marking(&exec, P_PROPOSAL);
    let got = format!(
        "261={} 278={} 291={} 341={} 343={} 322={}",
        text_of(&proposals["lane-261"], "action"),
        text_of(&proposals["lane-278"], "action"),
        text_of(&proposals["lane-291"], "action"),
        text_of(&proposals["lane-341"], "action"),
        text_of(&proposals["lane-343"], "action"),
        text_of(&proposals["verify-322"], "action"),
    );
    assert_eq!(
        got, "261=cold 278=resume 291=resume 341=resume 343=resume 322=cold",
        "ruling 363/a3.b per-agent decisions under the ruled idle clock"
    );
}
