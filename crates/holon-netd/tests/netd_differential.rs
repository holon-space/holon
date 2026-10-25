//! Differential: a random sequence of puts and fires through netd's MCP
//! endpoint ends in the same marking, and is refused at the same steps, as
//! the same sequence on the executor directly.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use common::Cli;
use holon_net::exec::Content;
use holon_net::exec::Executor;
use holon_net::exec::OccurrenceId;
use holon_net::file::NetFile;
use holon_net::load::net_def;
use holon_netd::Config;
use holon_pattern::Value as PValue;
use proptest::prelude::*;
use serde_json::Value;
use serde_json::json;

const NET: &str = "name: flow
places:
  A:
    capacity: single
  B:
    capacity: fifo
  C:
    capacity: register
  D:
    capacity: single
transitions:
  AtoB:
    trigger: manual
    priority: 1
    consumes:
    - A
    produces:
    - B
  BtoC:
    trigger: manual
    priority: 1
    consumes:
    - B
    inhibits:
    - D
    produces:
    - C
  CtoD:
    trigger: auto
    priority: 2
    reads:
    - C
    inhibits:
    - D
    produces:
    - D
  DropD:
    trigger: manual
    priority: 1
    consumes:
    - D
";

const PLACES: [&str; 4] = ["A", "B", "C", "D"];
const TRANSITIONS: [&str; 4] = ["AtoB", "BtoC", "CtoD", "DropD"];
const SUBJECTS: [&str; 3] = ["s1", "s2", "s3"];

#[derive(Clone, Debug)]
enum Op {
    Put {
        place: &'static str,
        subject: &'static str,
        n: i64,
    },
    Fire {
        transition: &'static str,
        subject: &'static str,
    },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (
            prop::sample::select(&PLACES[..]),
            prop::sample::select(&SUBJECTS[..]),
            0..3i64
        )
            .prop_map(|(place, subject, n)| Op::Put { place, subject, n }),
        (
            prop::sample::select(&TRANSITIONS[..]),
            prop::sample::select(&SUBJECTS[..])
        )
            .prop_map(|(transition, subject)| Op::Fire {
                transition,
                subject
            }),
    ]
}

/// What the direct executor answers to each op, and its marking at the end.
fn direct(ops: &[Op]) -> (Vec<bool>, Value) {
    let file = NetFile::parse(NET).expect("the net parses");
    let net = net_def("flow.yaml", NET, |t| {
        holon_netd::logic(&file.transitions[t])
    })
    .unwrap_or_else(|e| panic!("the net loads: {e}"));
    let Ok(mut exec) = Executor::new(net) else {
        panic!("the net checks");
    };
    let now = Duration::ZERO;
    let answers = ops
        .iter()
        .enumerate()
        .map(|(i, op)| match op {
            Op::Put { place, subject, n } => {
                let content = Content::from([("n".to_owned(), PValue::Integer(*n))]);
                exec.write(place, subject, content, now).is_ok()
            }
            Op::Fire {
                transition,
                subject,
            } => {
                let occurrence = OccurrenceId::caller(&format!("op-{i}"));
                exec.fire(transition, subject, occurrence, Content::new(), now)
                    .is_ok()
            }
        })
        .collect();
    let places: serde_json::Map<String, Value> = PLACES
        .iter()
        .map(|place| {
            let rows: serde_json::Map<String, Value> = exec
                .tokens()
                .subjects(place)
                .into_iter()
                .map(|s| {
                    let contents = exec.tokens().contents(place, &s);
                    let json = serde_json::to_value(contents).expect("contents are JSON");
                    (s.to_string(), json)
                })
                .collect();
            ((*place).to_owned(), Value::Object(rows))
        })
        .collect();
    (answers, Value::Object(places))
}

async fn through_netd(ops: &[Op]) -> (Vec<bool>, Value) {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("flow.yaml");
    std::fs::write(&path, NET).expect("the net file is writable");
    let config = Config {
        listen: "127.0.0.1:0".parse().expect("a loopback address"),
        state_dir: PathBuf::from(dir.path()),
        nets: vec![path],
    };
    let netd = holon_netd::start(config)
        .await
        .unwrap_or_else(|e| panic!("netd starts: {e}"));
    let cli = Cli::connect(&netd.url()).await;
    let mut answers = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        let args = match op {
            Op::Put { place, subject, n } => (
                "put",
                json!({"net": "flow", "place": place, "subject": subject, "content": {"n": n}}),
            ),
            Op::Fire {
                transition,
                subject,
            } => (
                "fire",
                json!({"net": "flow", "transition": transition, "subject": subject,
                       "occurrence": format!("op-{i}")}),
            ),
        };
        answers.push(cli.call(args.0, args.1).await.is_ok());
    }
    let marking = cli.json("marking", json!({"net": "flow"})).await;
    netd.shutdown().await;
    (answers, marking["places"].clone())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    #[test]
    fn netd_answers_and_marks_as_the_executor_does(ops in prop::collection::vec(op(), 1..16)) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("a tokio runtime");
        let got = runtime.block_on(through_netd(&ops));
        let want = direct(&ops);
        prop_assert_eq!(got.0, want.0, "which ops were refused");
        prop_assert_eq!(got.1, want.1, "the marking");
    }
}
