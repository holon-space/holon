//! netd: a long-running local host for standalone nets
//! (`docs/Architecture/NetHost.md`). It loads net files, runs each net's
//! executor with its shell effects and probes, and serves an MCP endpoint on
//! the loopback that puts facts, fires transitions and reads the marking.
//!
//! The marking lives in memory only: a start runs every probe once before
//! it serves, so after a restart the marking is what the probes report.

mod host;
mod tools;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use holon_net::exec::Content;
use holon_net::exec::LoadError;
use holon_net::exec::Output;
use holon_net::exec::Reader;
use holon_net::exec::Subject;
use holon_net::file::TransitionSpec;
use holon_net::load::Logic;
use holon_net::load::NetLoadError;
use holon_net_shell::effect::ShellError;
pub use host::HostedNet;
use rmcp::transport::streamable_http_server::StreamableHttpServerConfig;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

pub struct Config {
    /// A loopback address; port 0 picks a free one.
    pub listen: SocketAddr,
    /// Where each net's shell effects keep their pid file.
    pub state_dir: PathBuf,
    pub nets: Vec<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum NetdError {
    #[error("netd listens on the loopback only, not on {0}")]
    NotLoopback(SocketAddr),
    #[error(transparent)]
    NetFile(#[from] NetLoadError),
    #[error("{path}: the executor refuses the net: {source}")]
    Net { path: String, source: LoadError },
    #[error("{path}: the net `{name}` is already loaded from {first}")]
    DuplicateNet {
        path: String,
        name: String,
        first: String,
    },
    #[error("the state directory {path} is unusable: {source}")]
    StateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("the net `{net}`: {source}")]
    Shell { net: String, source: ShellError },
    #[error("the net `{net}` stopped: {message}")]
    Stopped { net: String, message: String },
    #[error("cannot listen on {addr}: {source}")]
    Listen {
        addr: SocketAddr,
        source: std::io::Error,
    },
    #[error("the MCP endpoint on {addr} stopped: {source}")]
    Serve {
        addr: SocketAddr,
        source: std::io::Error,
    },
}

/// The guard and firing netd gives every transition: the arcs alone decide
/// whether it fires (`docs/Architecture/NetFileFormat.md`, "Guards"), and a
/// firing puts one empty token into each `produces` and `facts` place.
pub fn logic(spec: &TransitionSpec) -> Logic {
    let bind = Arc::new(|_: &mut Reader<'_>, _: &Subject| Some(Content::new()));
    match spec.effect {
        Some(_) => Logic::effect(bind),
        None => {
            let outputs = outputs(spec);
            Logic::pure(
                bind,
                Arc::new(move |s: &Subject, _: &Content| Ok(outputs(s))),
            )
        }
    }
}

/// The outputs of one firing of `spec` for a subject.
pub(crate) fn outputs(
    spec: &TransitionSpec,
) -> impl Fn(&Subject) -> Vec<Output> + Send + Sync + use<> {
    let places: Vec<String> = spec.produces.iter().chain(&spec.facts).cloned().collect();
    move |s| {
        places
            .iter()
            .map(|p| Output::new(p, s, Content::new()))
            .collect()
    }
}

/// A running netd: its nets and its MCP endpoint.
pub struct Netd {
    addr: SocketAddr,
    cancel: CancellationToken,
    tasks: JoinSet<Result<(), NetdError>>,
}

/// Loads every net, runs its probes once, then listens on `config.listen`.
pub async fn start(config: Config) -> Result<Netd, NetdError> {
    if !config.listen.ip().is_loopback() {
        return Err(NetdError::NotLoopback(config.listen));
    }
    std::fs::create_dir_all(&config.state_dir).map_err(|source| NetdError::StateDir {
        path: config.state_dir.clone(),
        source,
    })?;
    let mut nets: BTreeMap<String, Arc<HostedNet>> = BTreeMap::new();
    let mut from: BTreeMap<String, String> = BTreeMap::new();
    for path in &config.nets {
        let net = HostedNet::load(path, &config.state_dir)?;
        let shown = path.display().to_string();
        if let Some(first) = from.insert(net.name().to_owned(), shown.clone()) {
            return Err(NetdError::DuplicateNet {
                path: shown,
                name: net.name().to_owned(),
                first,
            });
        }
        nets.insert(net.name().to_owned(), Arc::new(net));
    }
    for net in nets.values() {
        net.probe_once().await?;
    }
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|source| NetdError::Listen {
            addr: config.listen,
            source,
        })?;
    let addr = listener.local_addr().map_err(|source| NetdError::Listen {
        addr: config.listen,
        source,
    })?;
    let cancel = CancellationToken::new();
    let mut tasks = JoinSet::new();
    for net in nets.values() {
        let net = Arc::clone(net);
        tasks.spawn(async move { net.drive().await });
    }
    let nets = Arc::new(nets);
    let service = StreamableHttpService::new(
        move || Ok(tools::Tools::new(Arc::clone(&nets))),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig {
            sse_keep_alive: Some(std::time::Duration::from_secs(15)),
            stateful_mode: true,
            cancellation_token: cancel.child_token(),
        },
    );
    let router = axum::Router::new().nest_service("/mcp", service);
    let stop = cancel.clone();
    tasks.spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move { stop.cancelled().await })
            .await
            .map_err(|source| NetdError::Serve { addr, source })
    });
    Ok(Netd {
        addr,
        cancel,
        tasks,
    })
}

impl Netd {
    pub fn url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }

    /// Runs until a net or the endpoint stops, which only an error does.
    pub async fn run(mut self) -> NetdError {
        let ended = self
            .tasks
            .join_next()
            .await
            .expect("netd runs its endpoint as a task")
            .expect("a netd task does not panic");
        self.cancel.cancel();
        ended.expect_err("a net and the endpoint stop only on an error or a shutdown")
    }

    /// Stops the endpoint and every net. A shell effect still running is
    /// left to its process group and named in the pid file.
    pub async fn shutdown(mut self) {
        self.cancel.cancel();
        self.tasks.abort_all();
        while self.tasks.join_next().await.is_some() {}
    }
}
