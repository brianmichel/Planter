use std::{path::PathBuf, process::ExitCode};

use clap::Parser;
use planter_execd::{WorkerConfig, control_stream_from_fd, serve_control_stream};

/// CLI arguments for launching a `planter-execd` worker process.
#[derive(Debug, Parser)]
#[command(name = "planter-execd", about = "Planter sandboxed execution worker")]
struct Args {
    /// Inherited UNIX socket fd used for daemon control RPC.
    #[arg(long)]
    control_fd: i32,
    /// Shared auth token expected in the hello request.
    #[arg(long)]
    auth_token: String,
    /// Logical cell id assigned by the daemon.
    #[arg(long)]
    cell_id: String,
    /// Root state directory for worker data.
    #[arg(long)]
    state_root: PathBuf,
}

/// Entrypoint that maps worker startup failures to process exit code.
#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("planter-execd error: {err}");
            ExitCode::from(1)
        }
    }
}

/// Parses args, prepares worker config, and serves the control stream.
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_target(false).init();
    let args = Args::parse();

    tracing::info!(state_root = %args.state_root.display(), "starting planter-execd");

    let stream = control_stream_from_fd(args.control_fd)?;
    let config = WorkerConfig {
        cell_id: args.cell_id,
        auth_token: args.auth_token,
        state_root: args.state_root,
    };
    serve_control_stream(stream, config).await?;
    Ok(())
}
