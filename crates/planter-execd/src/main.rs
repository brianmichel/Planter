use std::{path::PathBuf, process::ExitCode};

use clap::Parser;
use planter_execd::{WorkerConfig, control_stream_from_fd, serve_control_stream};

#[derive(Debug, Parser)]
#[command(name = "planter-execd", about = "Planter sandboxed execution worker")]
struct Args {
    #[arg(long)]
    control_fd: i32,
    #[arg(long)]
    auth_token: String,
    #[arg(long)]
    cell_id: String,
    #[arg(long)]
    state_root: PathBuf,
}

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
