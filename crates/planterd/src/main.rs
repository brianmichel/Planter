mod dispatch;
mod handlers;

use std::{
    fs, io,
    os::unix::fs::FileTypeExt,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

use clap::Parser;
use dispatch::DaemonDispatcher;
use planter_core::PROTOCOL_VERSION;
use planter_ipc::serve_unix;
use tracing::info;

#[derive(Debug, Parser)]
#[command(name = "planterd", about = "Planter daemon")]
struct Args {
    #[arg(long, default_value = "/tmp/planterd.sock")]
    socket: PathBuf,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("planterd error: {err}");
            ExitCode::from(1)
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_target(false).init();

    let args = Args::parse();
    prepare_socket_path(&args.socket)?;

    info!(
        socket = %args.socket.display(),
        daemon = env!("CARGO_PKG_VERSION"),
        protocol = PROTOCOL_VERSION,
        "starting planterd"
    );

    serve_unix(&args.socket, Arc::new(DaemonDispatcher)).await?;
    Ok(())
}

fn prepare_socket_path(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_socket() {
                fs::remove_file(path)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{} exists and is not a socket", path.display()),
                ))
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}
