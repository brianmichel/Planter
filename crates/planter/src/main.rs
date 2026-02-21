use std::{path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};
use planter_core::{Request, Response};
use planter_ipc::PlanterClient;
use thiserror::Error;

#[derive(Debug, Parser)]
#[command(name = "planter", about = "Planter CLI")]
struct Cli {
    #[arg(long, default_value = "/tmp/planterd.sock")]
    socket: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Version,
    Health,
}

#[derive(Debug, Error)]
enum CliError {
    #[error(transparent)]
    Ipc(#[from] planter_ipc::IpcError),
    #[error("daemon error [{code:?}]: {message}{detail}")]
    Daemon {
        code: planter_core::ErrorCode,
        message: String,
        detail: String,
    },
    #[error("unexpected response for {command}: {response:?}")]
    Unexpected {
        command: &'static str,
        response: Response,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

async fn run() -> Result<(), CliError> {
    let cli = Cli::parse();
    let mut client = PlanterClient::connect(&cli.socket).await?;

    match cli.command {
        Command::Version => {
            let response = client.call(Request::Version {}).await?;
            match response {
                Response::Version { daemon, protocol } => {
                    println!("planterd {daemon} (protocol {protocol})");
                    Ok(())
                }
                Response::Error {
                    code,
                    message,
                    detail,
                } => Err(CliError::Daemon {
                    code,
                    message,
                    detail: format_detail(detail),
                }),
                other => Err(CliError::Unexpected {
                    command: "version",
                    response: other,
                }),
            }
        }
        Command::Health => {
            let response = client.call(Request::Health {}).await?;
            match response {
                Response::Health { status } => {
                    println!("{status}");
                    Ok(())
                }
                Response::Error {
                    code,
                    message,
                    detail,
                } => Err(CliError::Daemon {
                    code,
                    message,
                    detail: format_detail(detail),
                }),
                other => Err(CliError::Unexpected {
                    command: "health",
                    response: other,
                }),
            }
        }
    }
}

fn format_detail(detail: Option<String>) -> String {
    detail
        .map(|value| format!(" ({value})"))
        .unwrap_or_default()
}
