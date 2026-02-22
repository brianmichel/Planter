mod dispatch;
mod handlers;
mod state;
mod worker;
mod worker_manager;

use std::{
    fs, io,
    os::unix::fs::FileTypeExt,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

use clap::{Parser, ValueEnum};
use dispatch::DaemonDispatcher;
use planter_core::{PROTOCOL_VERSION, default_state_dir};
use planter_ipc::serve_unix;
use planter_platform::PlatformOps;
use state::StateStore;
use tracing::info;

#[cfg(target_os = "macos")]
use planter_platform_macos::{MacosOps, SandboxMode};

#[derive(Debug, Parser)]
#[command(name = "planterd", about = "Planter daemon")]
struct Args {
    #[arg(long, default_value = "/tmp/planterd.sock")]
    socket: PathBuf,
    #[arg(long, value_enum, default_value_t = SandboxModeArg::Permissive)]
    sandbox_mode: SandboxModeArg,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SandboxModeArg {
    Disabled,
    Permissive,
    Enforced,
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

    let state_dir = default_state_dir();
    let platform = select_platform(state_dir.clone(), args.sandbox_mode)?;
    let state = Arc::new(StateStore::new(state_dir, platform)?);

    info!(
        socket = %args.socket.display(),
        state_dir = %state.root().display(),
        sandbox_mode = %args.sandbox_mode.as_str(),
        daemon = env!("CARGO_PKG_VERSION"),
        protocol = PROTOCOL_VERSION,
        "starting planterd"
    );

    serve_unix(&args.socket, Arc::new(DaemonDispatcher::from(state))).await?;
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

#[cfg(target_os = "macos")]
fn select_platform(root: PathBuf, mode: SandboxModeArg) -> Result<Arc<dyn PlatformOps>, io::Error> {
    let sandbox_mode = match mode {
        SandboxModeArg::Disabled => SandboxMode::Disabled,
        SandboxModeArg::Permissive => SandboxMode::Permissive,
        SandboxModeArg::Enforced => SandboxMode::Enforced,
    };
    Ok(Arc::new(MacosOps::new(root, sandbox_mode)))
}

#[cfg(not(target_os = "macos"))]
fn select_platform(
    _root: PathBuf,
    _mode: SandboxModeArg,
) -> Result<Arc<dyn PlatformOps>, io::Error> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "no platform backend configured for this target",
    ))
}

impl SandboxModeArg {
    fn as_str(self) -> &'static str {
        match self {
            SandboxModeArg::Disabled => "disabled",
            SandboxModeArg::Permissive => "permissive",
            SandboxModeArg::Enforced => "enforced",
        }
    }
}
