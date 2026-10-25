use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use tokio::signal::unix::SignalKind;
use tokio::signal::unix::signal;

/// Hosts net files and serves them over MCP on the loopback.
#[derive(Parser)]
struct Args {
    /// The loopback address of the MCP endpoint; port 0 picks a free one.
    #[arg(long)]
    listen: SocketAddr,
    /// Where the shell effects keep their pid files.
    #[arg(long)]
    state_dir: PathBuf,
    /// The net files to host.
    #[arg(required = true)]
    nets: Vec<PathBuf>,
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    let config = holon_netd::Config {
        listen: args.listen,
        state_dir: args.state_dir,
        nets: args.nets,
    };
    let netd = match holon_netd::start(config).await {
        Ok(netd) => netd,
        Err(e) => {
            eprintln!("netd: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut stdout = std::io::stdout();
    writeln!(stdout, "netd listening on {}", netd.url()).expect("stdout is writable");
    stdout.flush().expect("stdout is writable");
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM can be caught");
    let url = netd.url();
    tokio::select! {
        e = netd.run() => {
            eprintln!("netd: {e}");
            ExitCode::FAILURE
        }
        _ = tokio::signal::ctrl_c() => stopping(&url),
        _ = term.recv() => stopping(&url),
    }
}

fn stopping(url: &str) -> ExitCode {
    tracing::info!("netd on {url} stops");
    ExitCode::SUCCESS
}
