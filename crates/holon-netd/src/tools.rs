//! The MCP tools of netd. Each answers with plain text for a reader and the
//! same answer as JSON in its structured content; a refusal is a tool error
//! that names what it refused.

use std::collections::BTreeMap;
use std::sync::Arc;

use holon_net::exec::Content;
use holon_net::exec::Holder;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Settled;
use holon_net::exec::WhyNot;
use holon_pattern::Value as PValue;
use rmcp::ErrorData as McpError;
use rmcp::ServerHandler;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::model::Implementation;
use rmcp::model::ServerCapabilities;
use rmcp::model::ServerInfo;
use rmcp::tool;
use rmcp::tool_handler;
use rmcp::tool_router;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;

use crate::HostedNet;

type Answer = Result<CallToolResult, McpError>;

#[derive(Clone)]
pub struct Tools {
    nets: Arc<BTreeMap<String, Arc<HostedNet>>>,
    tool_router: ToolRouter<Self>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct NetArgs {
    net: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct PutArgs {
    net: String,
    place: String,
    subject: String,
    /// The fact: a JSON object.
    content: Value,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct FireArgs {
    net: String,
    transition: String,
    subject: String,
    /// The caller's id for this firing; `await` takes it.
    occurrence: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct WhyNotArgs {
    net: String,
    transition: String,
    subject: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct AwaitArgs {
    net: String,
    occurrence: String,
}

fn answer(text: String, json: Value) -> Answer {
    let mut result = CallToolResult::structured(json);
    result.content = vec![rmcp::model::Content::text(text)];
    Ok(result)
}

fn refuse(message: String) -> Answer {
    Ok(CallToolResult::error(vec![rmcp::model::Content::text(
        message,
    )]))
}

#[tool_router]
impl Tools {
    pub fn new(nets: Arc<BTreeMap<String, Arc<HostedNet>>>) -> Self {
        Self {
            nets,
            tool_router: Self::tool_router(),
        }
    }

    fn net(&self, name: &str) -> Result<&HostedNet, String> {
        self.nets.get(name).map(|n| &**n).ok_or_else(|| {
            let hosted: Vec<&str> = self.nets.keys().map(String::as_str).collect();
            format!("no net `{name}`; netd hosts {}", hosted.join(", "))
        })
    }

    #[tool(description = "Put a fact into a place of a net, under a subject.")]
    fn put(&self, Parameters(args): Parameters<PutArgs>) -> Answer {
        let net = match self.net(&args.net) {
            Ok(net) => net,
            Err(e) => return refuse(e),
        };
        let Some(spec) = net.file.places.get(&args.place) else {
            return refuse(format!(
                "the net `{}` has no place `{}`",
                args.net, args.place
            ));
        };
        if spec.pool.is_some() {
            return refuse(format!(
                "`{}` is a pool: its tokens are counted, not put",
                args.place
            ));
        }
        let Value::Object(fields) = args.content else {
            return refuse(format!(
                "the fact for {} {} is not a JSON object: {}",
                args.place, args.subject, args.content
            ));
        };
        let content: Content = fields
            .into_iter()
            .map(|(k, v)| (k, PValue::from_json_value(v)))
            .collect();
        let put = net.shell.apply(|exec, now| {
            exec.write(&args.place, &args.subject, content, now)
                .map(|q| q.requests)
        });
        match put {
            Ok(()) => answer(
                format!("put {} {}", args.place, args.subject),
                json!({"place": args.place, "subject": args.subject}),
            ),
            Err(e) => refuse(format!("put {} {}: {e}", args.place, args.subject)),
        }
    }

    #[tool(
        description = "Fire a manual transition for a subject. Answers once the firing is \
                          accepted; `await` waits for its outcome."
    )]
    fn fire(&self, Parameters(args): Parameters<FireArgs>) -> Answer {
        let net = match self.net(&args.net) {
            Ok(net) => net,
            Err(e) => return refuse(e),
        };
        if !net.file.transitions.contains_key(&args.transition) {
            return refuse(format!(
                "the net `{}` has no transition `{}`",
                args.net, args.transition
            ));
        }
        let occurrence = OccurrenceId::caller(&args.occurrence);
        match net.shell.fire(&args.transition, &args.subject, occurrence) {
            Ok(_) => answer(
                format!(
                    "{} {} accepted as {}",
                    args.transition, args.subject, args.occurrence
                ),
                json!({"occurrence": args.occurrence}),
            ),
            Err(e) => refuse(format!("fire {} {}: {e}", args.transition, args.subject)),
        }
    }

    #[tool(
        description = "Show the marking of a net: every token of every declared place, \
                          and the conditions the net and its probes report."
    )]
    fn marking(&self, Parameters(args): Parameters<NetArgs>) -> Answer {
        let net = match self.net(&args.net) {
            Ok(net) => net,
            Err(e) => return refuse(e),
        };
        let mut lines = Vec::new();
        let places: serde_json::Map<String, Value> = net.shell.read(|exec| {
            net.file
                .places
                .keys()
                .map(|place| {
                    let rows: serde_json::Map<String, Value> = exec
                        .tokens()
                        .subjects(place)
                        .into_iter()
                        .map(|s| {
                            let contents = exec.tokens().contents(place, &s);
                            for c in &contents {
                                let c = serde_json::to_string(c).expect("contents are JSON");
                                lines.push(format!("{place} {s} {c}"));
                            }
                            let json = serde_json::to_value(contents).expect("contents are JSON");
                            (s.to_string(), json)
                        })
                        .collect();
                    (place.clone(), Value::Object(rows))
                })
                .collect()
        });
        let mut conditions: Vec<String> = net
            .shell
            .read(|exec| exec.conditions().iter().map(|c| c.to_string()).collect());
        conditions.extend(net.probes().conditions().iter().map(|c| c.to_string()));
        lines.extend(conditions.iter().map(|c| format!("condition: {c}")));
        answer(
            lines.join("\n"),
            json!({"net": args.net, "places": places, "conditions": conditions}),
        )
    }

    #[tool(description = "Say whether a transition is enabled for a subject, and if not, why.")]
    fn why_not(&self, Parameters(args): Parameters<WhyNotArgs>) -> Answer {
        let net = match self.net(&args.net) {
            Ok(net) => net,
            Err(e) => return refuse(e),
        };
        if !net.file.transitions.contains_key(&args.transition) {
            return refuse(format!(
                "the net `{}` has no transition `{}`",
                args.net, args.transition
            ));
        }
        let why = net
            .shell
            .read(|exec| exec.why_not(&args.transition, &args.subject));
        let head = format!("{} {}", args.transition, args.subject);
        match why {
            Err(e) => refuse(e.to_string()),
            Ok(None) => answer(format!("{head}: enabled"), json!({"enabled": true})),
            Ok(Some(why)) => {
                let (text, json) = why_not(&why);
                answer(
                    format!("{head}: {text}"),
                    json!({"enabled": false, "why_not": json}),
                )
            }
        }
    }

    #[tool(
        name = "await",
        description = "Wait until an occurrence settles and show its outcome."
    )]
    async fn wait(&self, Parameters(args): Parameters<AwaitArgs>) -> Answer {
        let net = match self.net(&args.net) {
            Ok(net) => net,
            Err(e) => return refuse(e),
        };
        let occurrence = OccurrenceId::caller(&args.occurrence);
        let settled = match net.shell.settled(&occurrence).await {
            Ok(settled) => settled,
            Err(e) => return refuse(e.to_string()),
        };
        let (kind, cause) = match &settled {
            Settled::Done => ("done", None),
            Settled::Failed(c) => ("failed", Some(&c.text)),
            Settled::OutcomeUnknown(c) => ("outcome_unknown", Some(&c.text)),
            Settled::OverChangedInputs { cause, .. } => ("over_changed_inputs", Some(&cause.text)),
        };
        let text = match cause {
            Some(cause) => format!("{kind}: {cause}"),
            None => kind.to_owned(),
        };
        answer(
            text,
            json!({"occurrence": args.occurrence, "settled": kind, "cause": cause}),
        )
    }
}

fn why_not(why: &WhyNot) -> (String, Value) {
    match why {
        WhyNot::InFlight => (
            "a firing is in flight".to_owned(),
            json!({"kind": "in_flight"}),
        ),
        WhyNot::RetryPending { due } => (
            format!("a retry is due at {due:?}"),
            json!({"kind": "retry_pending", "due_ms": due.as_millis()}),
        ),
        WhyNot::Needs { place, held_by } => {
            let holder = held_by.as_ref().map(|h| match h {
                Holder::Running(t) => format!("the running firing of {t}"),
                Holder::OutcomeUnknown(o) => format!("the occurrence {o} with an unknown outcome"),
            });
            let text = match &holder {
                Some(h) => format!("needs a token in {place}, held by {h}"),
                None => format!("needs a token in {place}"),
            };
            (
                text,
                json!({"kind": "needs", "place": place.as_str(), "held_by": holder}),
            )
        }
        WhyNot::Inhibited(place) => (
            format!("inhibited by a token in {place}"),
            json!({"kind": "inhibited", "place": place.as_str()}),
        ),
        WhyNot::Guard => ("its guard refuses".to_owned(), json!({"kind": "guard"})),
        WhyNot::FailedOnThisInput(cause) => (
            format!("it failed on this input: {}", cause.text),
            json!({"kind": "failed_on_this_input", "cause": cause.text}),
        ),
        WhyNot::ReadsUndeclared(place) => (
            format!("its bind reads {place}, which the net does not declare"),
            json!({"kind": "reads_undeclared", "place": place.as_str()}),
        ),
        WhyNot::BindsAnswerInput => (
            "its bind produces the field the answer value fills".to_owned(),
            json!({"kind": "binds_answer_input"}),
        ),
    }
}

#[tool_handler]
impl ServerHandler for Tools {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(format!(
                "netd hosts the nets {}. Put facts, fire manual transitions, read the marking, \
                 ask why a transition is not enabled, await an occurrence.",
                self.nets.keys().cloned().collect::<Vec<_>>().join(", ")
            )),
            ..ServerInfo::default()
        }
    }
}
