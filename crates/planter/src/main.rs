use std::{
    collections::BTreeMap,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
    time::Duration,
};

use clap::{Parser, Subcommand};
use planter_core::{
    CellId, CellSpec, CommandSpec, ErrorCode, Request, Response, default_state_dir,
};
use planter_ipc::PlanterClient;
use thiserror::Error;
use tokio::time::sleep;

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
    Create {
        #[arg(long)]
        name: String,
        #[arg(long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,
    },
    Run {
        cell_id: String,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,
        #[arg(last = true, required = true, num_args = 1..)]
        argv: Vec<String>,
    },
    Logs {
        job_id: String,
        #[arg(short = 'f', long)]
        follow: bool,
        #[arg(long)]
        stderr: bool,
    },
}

#[derive(Debug, Error)]
enum CliError {
    #[error(transparent)]
    Ipc(#[from] planter_ipc::IpcError),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("daemon error [{code:?}]: {message}{detail}")]
    Daemon {
        code: ErrorCode,
        message: String,
        detail: String,
    },
    #[error("invalid env var '{value}': expected KEY=VALUE")]
    InvalidEnv { value: String },
    #[error("unexpected response for {command}: {response:?}")]
    Unexpected {
        command: &'static str,
        response: Box<Response>,
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

    match cli.command {
        Command::Version => {
            let mut client = PlanterClient::connect(&cli.socket).await?;
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
                    response: Box::new(other),
                }),
            }
        }
        Command::Health => {
            let mut client = PlanterClient::connect(&cli.socket).await?;
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
                    response: Box::new(other),
                }),
            }
        }
        Command::Create { name, env } => {
            let mut client = PlanterClient::connect(&cli.socket).await?;
            let response = client
                .call(Request::CellCreate {
                    spec: CellSpec {
                        name,
                        env: parse_env_pairs(env)?,
                    },
                })
                .await?;

            match response {
                Response::CellCreated { cell } => {
                    println!("{}", cell.id.0);
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
                    command: "create",
                    response: Box::new(other),
                }),
            }
        }
        Command::Run {
            cell_id,
            cwd,
            env,
            argv,
        } => {
            let mut client = PlanterClient::connect(&cli.socket).await?;
            let response = client
                .call(Request::JobRun {
                    cell_id: CellId(cell_id),
                    cmd: CommandSpec {
                        argv,
                        cwd,
                        env: parse_env_pairs(env)?,
                    },
                })
                .await?;

            match response {
                Response::JobStarted { job } => {
                    println!("{}", job.id.0);
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
                    command: "run",
                    response: Box::new(other),
                }),
            }
        }
        Command::Logs {
            job_id,
            follow,
            stderr,
        } => stream_logs(&job_id, follow, stderr).await,
    }
}

async fn stream_logs(job_id: &str, follow: bool, stderr: bool) -> Result<(), CliError> {
    let path = default_state_dir().join("logs").join(format!(
        "{job_id}.{}.log",
        if stderr { "stderr" } else { "stdout" }
    ));

    let mut printed = 0_usize;

    loop {
        match std::fs::read(&path) {
            Ok(bytes) => {
                if printed < bytes.len() {
                    let mut stdout = io::stdout().lock();
                    stdout.write_all(&bytes[printed..])?;
                    stdout.flush()?;
                    printed = bytes.len();
                }

                if !follow {
                    return Ok(());
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound && follow => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(CliError::Io(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("log file not found: {}", path.display()),
                )));
            }
            Err(err) => return Err(CliError::Io(err)),
        }

        tokio::select! {
            _ = sleep(Duration::from_millis(250)) => {}
            _ = tokio::signal::ctrl_c() => return Ok(()),
        }
    }
}

fn parse_env_pairs(pairs: Vec<String>) -> Result<BTreeMap<String, String>, CliError> {
    let mut env = BTreeMap::new();

    for pair in pairs {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(CliError::InvalidEnv { value: pair });
        };

        if key.is_empty() {
            return Err(CliError::InvalidEnv {
                value: format!("={value}"),
            });
        }

        env.insert(key.to_string(), value.to_string());
    }

    Ok(env)
}

fn format_detail(detail: Option<String>) -> String {
    detail
        .map(|value| format!(" ({value})"))
        .unwrap_or_default()
}
