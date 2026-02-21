use std::{
    collections::BTreeMap,
    io::{self, Write},
    mem::MaybeUninit,
    os::fd::AsRawFd,
    path::PathBuf,
    process::ExitCode,
};

use clap::{Parser, Subcommand};
use planter_core::{
    CellId, CellSpec, CommandSpec, ErrorCode, ExitStatus, JobId, LogStream, Request, Response,
    SessionId,
};
use planter_ipc::PlanterClient;
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    task::JoinError,
};

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
        #[arg(long, default_value_t = 65536)]
        max_bytes: u32,
        #[arg(long, default_value_t = 1000)]
        wait_ms: u64,
    },
    Job {
        #[command(subcommand)]
        command: JobCommand,
    },
    Cell {
        #[command(subcommand)]
        command: CellCommand,
    },
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
}

#[derive(Debug, Subcommand)]
enum JobCommand {
    Status {
        job_id: String,
    },
    Kill {
        job_id: String,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
enum CellCommand {
    Rm {
        cell_id: String,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
enum SessionCommand {
    Open {
        #[arg(long, default_value = "/bin/zsh")]
        shell: String,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,
        #[arg(long, default_value_t = 120)]
        cols: u16,
        #[arg(long, default_value_t = 40)]
        rows: u16,
        #[arg(last = true)]
        args: Vec<String>,
    },
    Read {
        session_id: u64,
        #[arg(long, default_value_t = 0)]
        offset: u64,
        #[arg(long, default_value_t = 65536)]
        max_bytes: u32,
        #[arg(short = 'f', long)]
        follow: bool,
        #[arg(long, default_value_t = 250)]
        wait_ms: u64,
    },
    Write {
        session_id: u64,
        data: String,
    },
    Resize {
        session_id: u64,
        cols: u16,
        rows: u16,
    },
    Close {
        session_id: u64,
        #[arg(long)]
        force: bool,
    },
    Attach {
        session_id: u64,
        #[arg(long, default_value_t = 120)]
        cols: u16,
        #[arg(long, default_value_t = 40)]
        rows: u16,
    },
}

#[derive(Debug, Error)]
enum CliError {
    #[error(transparent)]
    Ipc(#[from] planter_ipc::IpcError),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("task join error: {0}")]
    Join(#[from] JoinError),
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
                    response: Box::new(other),
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
                    response: Box::new(other),
                }),
            }
        }
        Command::Create { name, env } => {
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
            let response = client
                .call(Request::JobRun {
                    cell_id: CellId(cell_id),
                    cmd: CommandSpec {
                        argv,
                        cwd,
                        env: parse_env_pairs(env)?,
                        limits: None,
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
            max_bytes,
            wait_ms,
        } => {
            stream_logs(
                &mut client,
                &JobId(job_id),
                if stderr {
                    LogStream::Stderr
                } else {
                    LogStream::Stdout
                },
                follow,
                max_bytes,
                wait_ms,
            )
            .await
        }
        Command::Job { command } => match command {
            JobCommand::Status { job_id } => {
                let response = client
                    .call(Request::JobStatus {
                        job_id: JobId(job_id),
                    })
                    .await?;
                match response {
                    Response::JobStatus { job } => {
                        let status = match job.status {
                            ExitStatus::Running => "running".to_string(),
                            ExitStatus::Exited { code } => {
                                format!(
                                    "exited({})",
                                    code.map_or_else(|| "none".to_string(), |c| c.to_string())
                                )
                            }
                        };
                        println!("{} {}", job.id.0, status);
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
                        command: "job status",
                        response: Box::new(other),
                    }),
                }
            }
            JobCommand::Kill { job_id, force } => {
                let response = client
                    .call(Request::JobKill {
                        job_id: JobId(job_id),
                        force,
                    })
                    .await?;
                match response {
                    Response::JobKilled {
                        job_id,
                        signal,
                        status,
                    } => {
                        println!("{} {} {:?}", job_id.0, signal, status);
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
                        command: "job kill",
                        response: Box::new(other),
                    }),
                }
            }
        },
        Command::Cell { command } => match command {
            CellCommand::Rm { cell_id, force } => {
                let response = client
                    .call(Request::CellRemove {
                        cell_id: CellId(cell_id),
                        force,
                    })
                    .await?;
                match response {
                    Response::CellRemoved { cell_id } => {
                        println!("{}", cell_id.0);
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
                        command: "cell rm",
                        response: Box::new(other),
                    }),
                }
            }
        },
        Command::Session { command } => match command {
            SessionCommand::Open {
                shell,
                cwd,
                env,
                cols,
                rows,
                args,
            } => {
                let response = client
                    .call(Request::PtyOpen {
                        shell,
                        args,
                        cwd,
                        env: parse_env_pairs(env)?,
                        cols,
                        rows,
                    })
                    .await?;
                match response {
                    Response::PtyOpened { session_id, pid } => {
                        match pid {
                            Some(pid) => println!("{} {}", session_id.0, pid),
                            None => println!("{}", session_id.0),
                        }
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
                        command: "session open",
                        response: Box::new(other),
                    }),
                }
            }
            SessionCommand::Read {
                session_id,
                offset,
                max_bytes,
                follow,
                wait_ms,
            } => {
                stream_pty(
                    &mut client,
                    SessionId(session_id),
                    offset,
                    max_bytes,
                    follow,
                    wait_ms,
                )
                .await
            }
            SessionCommand::Write { session_id, data } => {
                let response = client
                    .call(Request::PtyInput {
                        session_id: SessionId(session_id),
                        data: data.into_bytes(),
                    })
                    .await?;
                match response {
                    Response::PtyAck { .. } => Ok(()),
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
                        command: "session write",
                        response: Box::new(other),
                    }),
                }
            }
            SessionCommand::Resize {
                session_id,
                cols,
                rows,
            } => {
                let response = client
                    .call(Request::PtyResize {
                        session_id: SessionId(session_id),
                        cols,
                        rows,
                    })
                    .await?;
                match response {
                    Response::PtyAck { .. } => Ok(()),
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
                        command: "session resize",
                        response: Box::new(other),
                    }),
                }
            }
            SessionCommand::Close { session_id, force } => {
                let response = client
                    .call(Request::PtyClose {
                        session_id: SessionId(session_id),
                        force,
                    })
                    .await?;
                match response {
                    Response::PtyAck { .. } => Ok(()),
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
                        command: "session close",
                        response: Box::new(other),
                    }),
                }
            }
            SessionCommand::Attach {
                session_id,
                cols,
                rows,
            } => attach_session(&cli.socket, SessionId(session_id), cols, rows).await,
        },
    }
}

async fn stream_logs(
    client: &mut PlanterClient,
    job_id: &JobId,
    stream: LogStream,
    follow: bool,
    max_bytes: u32,
    wait_ms: u64,
) -> Result<(), CliError> {
    let mut offset: u64 = 0;

    loop {
        let response = client
            .call(Request::LogsRead {
                job_id: job_id.clone(),
                stream,
                offset,
                max_bytes,
                follow,
                wait_ms,
            })
            .await?;

        match response {
            Response::LogsChunk {
                data,
                eof,
                complete,
                ..
            } => {
                if !data.is_empty() {
                    let mut stdout = io::stdout().lock();
                    stdout.write_all(&data)?;
                    stdout.flush()?;
                    offset = offset.saturating_add(data.len() as u64);
                }

                if complete || (!follow && eof && data.is_empty()) {
                    return Ok(());
                }
            }
            Response::Error {
                code,
                message,
                detail,
            } => {
                return Err(CliError::Daemon {
                    code,
                    message,
                    detail: format_detail(detail),
                });
            }
            other => {
                return Err(CliError::Unexpected {
                    command: "logs",
                    response: Box::new(other),
                });
            }
        }
    }
}

async fn stream_pty(
    client: &mut PlanterClient,
    session_id: SessionId,
    mut offset: u64,
    max_bytes: u32,
    follow: bool,
    wait_ms: u64,
) -> Result<(), CliError> {
    loop {
        let response = client
            .call(Request::PtyRead {
                session_id,
                offset,
                max_bytes,
                follow,
                wait_ms,
            })
            .await?;

        match response {
            Response::PtyChunk {
                data,
                eof,
                complete,
                ..
            } => {
                if !data.is_empty() {
                    let mut stdout = io::stdout().lock();
                    stdout.write_all(&data)?;
                    stdout.flush()?;
                    offset = offset.saturating_add(data.len() as u64);
                }

                if complete || (!follow && eof && data.is_empty()) {
                    return Ok(());
                }
            }
            Response::Error {
                code,
                message,
                detail,
            } => {
                return Err(CliError::Daemon {
                    code,
                    message,
                    detail: format_detail(detail),
                });
            }
            other => {
                return Err(CliError::Unexpected {
                    command: "session read",
                    response: Box::new(other),
                });
            }
        }
    }
}

async fn attach_session(
    socket: &PathBuf,
    session_id: SessionId,
    cols: u16,
    rows: u16,
) -> Result<(), CliError> {
    print_planter_banner()?;
    let _terminal_mode = TerminalModeGuard::enter_raw()?;

    let mut control = PlanterClient::connect(socket).await?;
    let resize = control
        .call(Request::PtyResize {
            session_id,
            cols,
            rows,
        })
        .await?;
    match resize {
        Response::PtyAck { .. } => {}
        Response::Error {
            code,
            message,
            detail,
        } => {
            return Err(CliError::Daemon {
                code,
                message,
                detail: format_detail(detail),
            });
        }
        other => {
            return Err(CliError::Unexpected {
                command: "session attach resize",
                response: Box::new(other),
            });
        }
    }

    let mut read_client = PlanterClient::connect(socket).await?;
    let mut write_client = PlanterClient::connect(socket).await?;

    let mut read_task = tokio::spawn(async move {
        let mut offset = 0_u64;
        let mut stdout = tokio::io::stdout();
        loop {
            let response = read_client
                .call(Request::PtyRead {
                    session_id,
                    offset,
                    max_bytes: 65536,
                    follow: true,
                    wait_ms: 200,
                })
                .await?;

            match response {
                Response::PtyChunk { data, complete, .. } => {
                    if !data.is_empty() {
                        stdout.write_all(&data).await?;
                        stdout.flush().await?;
                        offset = offset.saturating_add(data.len() as u64);
                    }
                    if complete {
                        return Ok::<(), CliError>(());
                    }
                }
                Response::Error {
                    code,
                    message,
                    detail,
                } => {
                    return Err(CliError::Daemon {
                        code,
                        message,
                        detail: format_detail(detail),
                    });
                }
                other => {
                    return Err(CliError::Unexpected {
                        command: "session attach read",
                        response: Box::new(other),
                    });
                }
            }
        }
    });

    let mut write_task = tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let mut buf = vec![0_u8; 1024];
        loop {
            let read = stdin.read(&mut buf).await?;
            if read == 0 {
                match write_client
                    .call(Request::PtyClose {
                        session_id,
                        force: false,
                    })
                    .await
                {
                    Ok(Response::PtyAck { .. }) => {}
                    Ok(Response::Error {
                        code: ErrorCode::NotFound,
                        ..
                    }) => {}
                    Ok(Response::Error {
                        code,
                        message,
                        detail,
                    }) => {
                        return Err(CliError::Daemon {
                            code,
                            message,
                            detail: format_detail(detail),
                        });
                    }
                    Ok(other) => {
                        return Err(CliError::Unexpected {
                            command: "session attach close",
                            response: Box::new(other),
                        });
                    }
                    Err(err) => return Err(CliError::Ipc(err)),
                }
                return Ok::<(), CliError>(());
            }

            let response = write_client
                .call(Request::PtyInput {
                    session_id,
                    data: buf[..read].to_vec(),
                })
                .await?;
            match response {
                Response::PtyAck { .. } => {}
                Response::Error {
                    code,
                    message,
                    detail,
                } => {
                    return Err(CliError::Daemon {
                        code,
                        message,
                        detail: format_detail(detail),
                    });
                }
                other => {
                    return Err(CliError::Unexpected {
                        command: "session attach write",
                        response: Box::new(other),
                    });
                }
            }
        }
    });

    tokio::select! {
        result = &mut read_task => {
            write_task.abort();
            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    if !matches!(err, CliError::Daemon { code: ErrorCode::NotFound, .. }) {
                        return Err(err);
                    }
                }
                Err(err) => return Err(CliError::Join(err)),
            }
        }
        result = &mut write_task => {
            result??;
            match read_task.await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    if !matches!(err, CliError::Daemon { code: ErrorCode::NotFound, .. }) {
                        return Err(err);
                    }
                }
                Err(err) => return Err(CliError::Join(err)),
            }
        }
        _ = tokio::signal::ctrl_c() => {
            let mut close_client = PlanterClient::connect(socket).await?;
            let _ = close_client.call(Request::PtyClose { session_id, force: false }).await;
            read_task.abort();
            write_task.abort();
        }
    }

    Ok(())
}

fn print_planter_banner() -> Result<(), CliError> {
    const BANNER: &str = r#"
   .-.
  ( * )   .-.
   `-'   ( @ )
    |     `-'
\  |  /\  |  /\  |  /
 \ | /  \ | /  \ | /
  \|/ /\ \|/ /\ \|/
~~~~~~~~~~~~~~~~~~~~~~~~~~
     planter flower field
"#;

    let mut stdout = io::stdout().lock();
    stdout.write_all(BANNER.as_bytes())?;
    stdout.flush()?;
    Ok(())
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

struct TerminalModeGuard {
    fd: i32,
    original: Option<libc::termios>,
}

impl TerminalModeGuard {
    fn enter_raw() -> Result<Self, CliError> {
        let fd = io::stdin().as_raw_fd();

        // SAFETY: libc::isatty is a pure FFI call that does not retain pointers.
        if unsafe { libc::isatty(fd) } != 1 {
            return Ok(Self { fd, original: None });
        }

        let mut original = MaybeUninit::<libc::termios>::uninit();
        // SAFETY: fd is from stdin, and original points to valid writable memory.
        let get_attr = unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) };
        if get_attr != 0 {
            return Err(CliError::Io(io::Error::last_os_error()));
        }

        // SAFETY: tcgetattr succeeded, so original is initialized.
        let original = unsafe { original.assume_init() };
        let mut raw = original;
        // SAFETY: raw points to valid termios storage.
        unsafe { libc::cfmakeraw(&mut raw) };

        // SAFETY: fd and raw termios are valid for this process.
        let set_attr = unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) };
        if set_attr != 0 {
            return Err(CliError::Io(io::Error::last_os_error()));
        }

        Ok(Self {
            fd,
            original: Some(original),
        })
    }
}

impl Drop for TerminalModeGuard {
    fn drop(&mut self) {
        if let Some(original) = self.original {
            // SAFETY: fd and saved termios came from a successful tcgetattr call.
            let _ = unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &original) };
        }
    }
}
