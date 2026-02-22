mod pty;

use std::{
    collections::HashMap,
    fs,
    os::fd::{FromRawFd, RawFd},
    path::Path,
    process::{Command as StdCommand, Stdio},
    time::Duration,
};

use planter_core::{ErrorCode, ExitStatus, JobId, PlanterError, TerminationReason, now_ms};
use planter_execd_proto::{
    EXECD_PROTOCOL_VERSION, ExecErrorCode, ExecPtyAction, ExecRequest, ExecRequestEnvelope,
    ExecResponse, ExecResponseEnvelope,
};
use planter_ipc::{
    IpcError,
    codec::{decode, encode},
    framing::{read_frame, write_frame},
};
use thiserror::Error;
use tokio::{net::UnixStream, process::Child, process::Command, time::sleep};

use crate::pty::{PtyManager, PtySandboxMode};

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub cell_id: String,
    pub auth_token: String,
    pub state_root: std::path::PathBuf,
}

#[derive(Debug, Error)]
pub enum WorkerError {
    #[error(transparent)]
    Ipc(#[from] IpcError),
    #[error("invalid control socket fd {fd}: {source}")]
    InvalidControlFd { fd: RawFd, source: std::io::Error },
    #[error("failed to convert worker control stream: {0}")]
    ControlStream(std::io::Error),
}

struct WorkerRuntime {
    jobs: HashMap<JobId, WorkerJob>,
    pty: PtyManager,
}

struct WorkerJob {
    child: Child,
    status: ExitStatus,
    finished_at_ms: Option<u64>,
    termination_reason: Option<TerminationReason>,
}

pub fn control_stream_from_fd(fd: RawFd) -> Result<UnixStream, WorkerError> {
    if fd < 0 {
        return Err(WorkerError::InvalidControlFd {
            fd,
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "fd must be >= 0"),
        });
    }
    // SAFETY: fd comes from the parent process and is expected to be a valid unix socket fd.
    let std_stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) };
    std_stream
        .set_nonblocking(true)
        .map_err(|source| WorkerError::InvalidControlFd { fd, source })?;
    UnixStream::from_std(std_stream).map_err(WorkerError::ControlStream)
}

pub async fn serve_control_stream(
    mut stream: UnixStream,
    config: WorkerConfig,
) -> Result<(), WorkerError> {
    let mut authed = false;
    let mut runtime = WorkerRuntime::new(config.state_root.clone());

    loop {
        let frame = read_frame(&mut stream).await?;
        let request: ExecRequestEnvelope = decode(&frame)?;
        let req_id = request.req_id;

        if !authed {
            let response = match request.body {
                ExecRequest::Hello {
                    protocol,
                    auth_token,
                    cell_id,
                } => {
                    if protocol != EXECD_PROTOCOL_VERSION {
                        ExecResponse::ExecError {
                            code: ExecErrorCode::InvalidRequest,
                            message: "unsupported exec protocol version".to_string(),
                            detail: Some(format!(
                                "expected={EXECD_PROTOCOL_VERSION} got={protocol}"
                            )),
                        }
                    } else if auth_token != config.auth_token {
                        ExecResponse::ExecError {
                            code: ExecErrorCode::Unauthorized,
                            message: "invalid worker auth token".to_string(),
                            detail: None,
                        }
                    } else if cell_id != config.cell_id {
                        ExecResponse::ExecError {
                            code: ExecErrorCode::InvalidRequest,
                            message: "worker cell mismatch".to_string(),
                            detail: Some(format!("expected={} got={}", config.cell_id, cell_id)),
                        }
                    } else {
                        authed = true;
                        ExecResponse::HelloAck {
                            protocol: EXECD_PROTOCOL_VERSION,
                            worker_pid: std::process::id(),
                        }
                    }
                }
                _ => ExecResponse::ExecError {
                    code: ExecErrorCode::Unauthorized,
                    message: "hello required before other requests".to_string(),
                    detail: None,
                },
            };

            write_response(&mut stream, req_id, response).await?;
            if !authed {
                return Ok(());
            }
            continue;
        }

        let (response, should_exit) = runtime.handle_request(request.body).await;
        write_response(&mut stream, req_id, response).await?;
        if should_exit {
            return Ok(());
        }
    }
}

impl WorkerRuntime {
    fn new(state_root: std::path::PathBuf) -> Self {
        Self {
            jobs: HashMap::new(),
            pty: PtyManager::new(state_root, PtySandboxMode::Disabled),
        }
    }

    async fn handle_request(&mut self, request: ExecRequest) -> (ExecResponse, bool) {
        match request {
            ExecRequest::Ping {} => (ExecResponse::Pong {}, false),
            ExecRequest::RunJob {
                job_id,
                cmd,
                env,
                stdout_path,
                stderr_path,
            } => {
                let result = self
                    .run_job(job_id, cmd, env, stdout_path, stderr_path)
                    .await;
                (map_result(result), false)
            }
            ExecRequest::JobStatus { job_id } => {
                let result = self.job_status(job_id).await;
                (map_result(result), false)
            }
            ExecRequest::JobSignal { job_id, force } => {
                let result = self.job_signal(job_id, force).await;
                (map_result(result), false)
            }
            ExecRequest::PtyOpen {
                shell,
                args,
                cwd,
                env,
                cols,
                rows,
            } => {
                let result = self
                    .pty
                    .open(shell, args, cwd, env, cols, rows)
                    .map(|opened| ExecResponse::PtyOpened {
                        session_id: opened.session_id,
                        pid: opened.pid,
                    });
                (map_result(result), false)
            }
            ExecRequest::PtyInput { session_id, data } => {
                let result = self
                    .pty
                    .input(session_id, data)
                    .map(|()| ExecResponse::PtyAck {
                        session_id,
                        action: ExecPtyAction::Input,
                    });
                (map_result(result), false)
            }
            ExecRequest::PtyRead {
                session_id,
                offset,
                max_bytes,
                follow,
                wait_ms,
            } => {
                let result = self
                    .pty
                    .read(session_id, offset, max_bytes, follow, wait_ms)
                    .await
                    .map(|chunk| ExecResponse::PtyChunk {
                        session_id,
                        offset: chunk.offset,
                        data: chunk.data,
                        eof: chunk.eof,
                        complete: chunk.complete,
                        exit_code: chunk.exit_code,
                    });
                (map_result(result), false)
            }
            ExecRequest::PtyResize {
                session_id,
                cols,
                rows,
            } => {
                let result =
                    self.pty
                        .resize(session_id, cols, rows)
                        .map(|()| ExecResponse::PtyAck {
                            session_id,
                            action: ExecPtyAction::Resize,
                        });
                (map_result(result), false)
            }
            ExecRequest::PtyClose { session_id, force } => {
                let result = self
                    .pty
                    .close(session_id, force)
                    .map(|()| ExecResponse::PtyAck {
                        session_id,
                        action: ExecPtyAction::Closed,
                    });
                (map_result(result), false)
            }
            ExecRequest::UsageProbe { job_id } => {
                let result = self.usage_probe(job_id).await;
                (map_result(result), false)
            }
            ExecRequest::Shutdown { force } => {
                self.shutdown(force).await;
                (ExecResponse::Pong {}, true)
            }
            ExecRequest::Hello { .. } => (
                ExecResponse::ExecError {
                    code: ExecErrorCode::InvalidRequest,
                    message: "hello can only be sent once".to_string(),
                    detail: None,
                },
                false,
            ),
        }
    }

    async fn run_job(
        &mut self,
        job_id: JobId,
        cmd: planter_core::CommandSpec,
        env: std::collections::BTreeMap<String, String>,
        stdout_path: String,
        stderr_path: String,
    ) -> Result<ExecResponse, PlanterError> {
        if cmd.argv.is_empty() {
            return Err(PlanterError {
                code: ErrorCode::InvalidRequest,
                message: "command argv cannot be empty".to_string(),
                detail: None,
            });
        }

        if self.jobs.contains_key(&job_id) {
            return Err(PlanterError {
                code: ErrorCode::InvalidRequest,
                message: "job already exists".to_string(),
                detail: Some(job_id.0),
            });
        }

        ensure_parent_dir(&stdout_path)?;
        ensure_parent_dir(&stderr_path)?;
        let stdout_file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&stdout_path)
            .map_err(|err| io_to_planter_error("open stdout log", err))?;
        let stderr_file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&stderr_path)
            .map_err(|err| io_to_planter_error("open stderr log", err))?;

        let mut command = Command::new(&cmd.argv[0]);
        if cmd.argv.len() > 1 {
            command.args(&cmd.argv[1..]);
        }
        if let Some(cwd) = cmd.cwd {
            command.current_dir(cwd);
        }
        command.envs(env);
        command.stdout(Stdio::from(stdout_file));
        command.stderr(Stdio::from(stderr_file));

        let child = command
            .spawn()
            .map_err(|err| io_to_planter_error("spawn job", err))?;
        let pid = child.id();

        self.jobs.insert(
            job_id.clone(),
            WorkerJob {
                child,
                status: ExitStatus::Running,
                finished_at_ms: None,
                termination_reason: None,
            },
        );

        Ok(ExecResponse::JobStarted { job_id, pid })
    }

    async fn job_status(&mut self, job_id: JobId) -> Result<ExecResponse, PlanterError> {
        let job = self.get_job_mut(&job_id)?;
        refresh_job(job)?;
        Ok(ExecResponse::JobStatus {
            job_id,
            status: job.status.clone(),
            finished_at_ms: job.finished_at_ms,
            termination_reason: job.termination_reason,
        })
    }

    async fn job_signal(
        &mut self,
        job_id: JobId,
        force: bool,
    ) -> Result<ExecResponse, PlanterError> {
        let job = self.get_job_mut(&job_id)?;
        if matches!(job.status, ExitStatus::Running) {
            signal_job(&mut job.child, force).await;
            job.status = ExitStatus::Exited { code: None };
            job.finished_at_ms = Some(now_ms());
            job.termination_reason = Some(if force {
                TerminationReason::ForcedKill
            } else {
                TerminationReason::TerminatedByUser
            });
        }
        Ok(ExecResponse::JobStatus {
            job_id,
            status: job.status.clone(),
            finished_at_ms: job.finished_at_ms,
            termination_reason: job.termination_reason,
        })
    }

    async fn usage_probe(&mut self, job_id: JobId) -> Result<ExecResponse, PlanterError> {
        let job = self.get_job_mut(&job_id)?;
        refresh_job(job)?;
        let pid = job.child.id();
        let rss_bytes = pid.and_then(|pid| read_rss_bytes(pid).ok().flatten());

        Ok(ExecResponse::UsageSample {
            job_id,
            rss_bytes,
            cpu_nanos: None,
            timestamp_ms: now_ms(),
        })
    }

    async fn shutdown(&mut self, force: bool) {
        for job in self.jobs.values_mut() {
            if matches!(job.status, ExitStatus::Running) {
                signal_job(&mut job.child, force).await;
                job.status = ExitStatus::Exited { code: None };
                job.finished_at_ms = Some(now_ms());
                job.termination_reason = Some(if force {
                    TerminationReason::ForcedKill
                } else {
                    TerminationReason::TerminatedByUser
                });
            }
        }
    }

    fn get_job_mut(&mut self, job_id: &JobId) -> Result<&mut WorkerJob, PlanterError> {
        self.jobs.get_mut(job_id).ok_or_else(|| PlanterError {
            code: ErrorCode::NotFound,
            message: "job does not exist".to_string(),
            detail: Some(job_id.0.clone()),
        })
    }
}

fn map_result(result: Result<ExecResponse, PlanterError>) -> ExecResponse {
    match result {
        Ok(response) => response,
        Err(err) => ExecResponse::ExecError {
            code: ExecErrorCode::from(err.code),
            message: err.message,
            detail: err.detail,
        },
    }
}

async fn write_response(
    stream: &mut UnixStream,
    req_id: u64,
    body: ExecResponse,
) -> Result<(), WorkerError> {
    let response = ExecResponseEnvelope { req_id, body };
    let payload = encode(&response)?;
    write_frame(stream, &payload).await?;
    Ok(())
}

fn ensure_parent_dir(path: &str) -> Result<(), PlanterError> {
    if let Some(parent) = Path::new(path).parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|err| io_to_planter_error("create parent dir", err))?;
    }
    Ok(())
}

fn refresh_job(job: &mut WorkerJob) -> Result<(), PlanterError> {
    if !matches!(job.status, ExitStatus::Running) {
        return Ok(());
    }

    if let Some(status) = job
        .child
        .try_wait()
        .map_err(|err| io_to_planter_error("probe job status", err))?
    {
        job.status = ExitStatus::Exited {
            code: status.code(),
        };
        job.finished_at_ms = Some(now_ms());
        if job.termination_reason.is_none() {
            job.termination_reason = Some(TerminationReason::Exited);
        }
    }

    Ok(())
}

async fn signal_job(child: &mut Child, force: bool) {
    if let Some(pid) = child.id() {
        if force {
            let _ = send_signal(pid, "KILL");
            let _ = send_signal_to_children(pid, "KILL");
        } else {
            let _ = send_signal(pid, "TERM");
            let _ = send_signal_to_children(pid, "TERM");
            sleep(Duration::from_millis(250)).await;
            if process_alive(pid).unwrap_or(false) {
                let _ = send_signal(pid, "KILL");
                let _ = send_signal_to_children(pid, "KILL");
            }
        }
    } else {
        let _ = child.start_kill();
    }
}

fn send_signal(pid: u32, signal: &str) -> Result<(), std::io::Error> {
    let status = StdCommand::new("/bin/kill")
        .arg(format!("-{signal}"))
        .arg(pid.to_string())
        .status()?;
    if status.success() || status.code() == Some(1) {
        return Ok(());
    }
    Err(std::io::Error::other(format!(
        "kill -{signal} {pid} failed with status {status}"
    )))
}

fn send_signal_to_children(pid: u32, signal: &str) -> Result<(), std::io::Error> {
    let status = StdCommand::new("/usr/bin/pkill")
        .arg(format!("-{signal}"))
        .arg("-P")
        .arg(pid.to_string())
        .status()?;
    if status.success() || status.code() == Some(1) {
        return Ok(());
    }
    Err(std::io::Error::other(format!(
        "pkill -{signal} -P {pid} failed with status {status}"
    )))
}

fn process_alive(pid: u32) -> Result<bool, std::io::Error> {
    let status = StdCommand::new("/bin/kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()?;
    Ok(status.success())
}

fn read_rss_bytes(pid: u32) -> Result<Option<u64>, std::io::Error> {
    let output = StdCommand::new("/bin/ps")
        .arg("-o")
        .arg("rss=")
        .arg("-p")
        .arg(pid.to_string())
        .output()?;
    if !output.status.success() {
        return Ok(None);
    }
    let rss_kb = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .ok();
    Ok(rss_kb.map(|v| v.saturating_mul(1024)))
}

fn io_to_planter_error(action: &str, err: std::io::Error) -> PlanterError {
    PlanterError {
        code: ErrorCode::Internal,
        message: action.to_string(),
        detail: Some(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{WorkerConfig, serve_control_stream};
    use planter_core::{CommandSpec, ExitStatus, JobId};
    use planter_execd_proto::{
        EXECD_PROTOCOL_VERSION, ExecErrorCode, ExecRequest, ExecRequestEnvelope, ExecResponse,
        ExecResponseEnvelope,
    };
    use planter_ipc::{
        codec::{decode, encode},
        framing::{read_frame, write_frame},
    };
    use std::os::fd::{FromRawFd, IntoRawFd};
    use tempfile::tempdir;
    use tokio::io::AsyncWriteExt;
    use tokio::net::UnixStream;

    async fn pair() -> (UnixStream, UnixStream) {
        let (left, right) =
            std::os::unix::net::UnixStream::pair().expect("create unix stream pair");
        left.set_nonblocking(true).expect("left nonblocking");
        right.set_nonblocking(true).expect("right nonblocking");
        (
            UnixStream::from_std(left).expect("tokio left"),
            UnixStream::from_std(right).expect("tokio right"),
        )
    }

    async fn send(stream: &mut UnixStream, req_id: u64, body: ExecRequest) -> ExecResponseEnvelope {
        let request = ExecRequestEnvelope { req_id, body };
        let payload = encode(&request).expect("encode request");
        write_frame(stream, &payload).await.expect("write frame");
        let frame = read_frame(stream).await.expect("read response frame");
        decode::<ExecResponseEnvelope>(&frame).expect("decode response")
    }

    #[tokio::test]
    async fn hello_and_ping_succeed() {
        let tmp = tempdir().expect("tempdir");
        let (server_stream, mut client_stream) = pair().await;
        let config = WorkerConfig {
            cell_id: "cell-123".to_string(),
            auth_token: "token-123".to_string(),
            state_root: tmp.path().join("state"),
        };
        let server = tokio::spawn(async move { serve_control_stream(server_stream, config).await });

        let hello = send(
            &mut client_stream,
            1,
            ExecRequest::Hello {
                protocol: EXECD_PROTOCOL_VERSION,
                auth_token: "token-123".to_string(),
                cell_id: "cell-123".to_string(),
            },
        )
        .await;
        assert_eq!(hello.req_id, 1);
        match hello.body {
            ExecResponse::HelloAck { protocol, .. } => {
                assert_eq!(protocol, EXECD_PROTOCOL_VERSION)
            }
            other => panic!("unexpected response: {other:?}"),
        }

        let ping = send(&mut client_stream, 2, ExecRequest::Ping {}).await;
        assert_eq!(ping.req_id, 2);
        assert_eq!(ping.body, ExecResponse::Pong {});

        let _ = client_stream.shutdown().await;
        server.abort();
    }

    #[tokio::test]
    async fn run_job_and_query_status() {
        let tmp = tempdir().expect("tempdir");
        let (server_stream, mut client_stream) = pair().await;
        let config = WorkerConfig {
            cell_id: "cell-123".to_string(),
            auth_token: "token-123".to_string(),
            state_root: tmp.path().join("state"),
        };
        let server = tokio::spawn(async move { serve_control_stream(server_stream, config).await });

        let _ = send(
            &mut client_stream,
            1,
            ExecRequest::Hello {
                protocol: EXECD_PROTOCOL_VERSION,
                auth_token: "token-123".to_string(),
                cell_id: "cell-123".to_string(),
            },
        )
        .await;

        let started = send(
            &mut client_stream,
            2,
            ExecRequest::RunJob {
                job_id: JobId("job-1".to_string()),
                cmd: CommandSpec {
                    argv: vec![
                        "/bin/sh".to_string(),
                        "-c".to_string(),
                        "echo hello".to_string(),
                    ],
                    cwd: None,
                    env: Default::default(),
                    limits: None,
                },
                env: Default::default(),
                stdout_path: tmp.path().join("stdout.log").display().to_string(),
                stderr_path: tmp.path().join("stderr.log").display().to_string(),
            },
        )
        .await;
        match started.body {
            ExecResponse::JobStarted { job_id, .. } => assert_eq!(job_id.0, "job-1"),
            other => panic!("unexpected response: {other:?}"),
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let status = send(
            &mut client_stream,
            3,
            ExecRequest::JobStatus {
                job_id: JobId("job-1".to_string()),
            },
        )
        .await;
        match status.body {
            ExecResponse::JobStatus { status, .. } => {
                assert!(matches!(
                    status,
                    ExitStatus::Exited { .. } | ExitStatus::Running
                ));
            }
            other => panic!("unexpected response: {other:?}"),
        }

        let _ = client_stream.shutdown().await;
        server.abort();
    }

    #[tokio::test]
    async fn rejects_wrong_auth_token() {
        let tmp = tempdir().expect("tempdir");
        let (server_stream, mut client_stream) = pair().await;
        let config = WorkerConfig {
            cell_id: "cell-123".to_string(),
            auth_token: "token-123".to_string(),
            state_root: tmp.path().join("state"),
        };
        let server = tokio::spawn(async move { serve_control_stream(server_stream, config).await });

        let hello = send(
            &mut client_stream,
            1,
            ExecRequest::Hello {
                protocol: EXECD_PROTOCOL_VERSION,
                auth_token: "bad-token".to_string(),
                cell_id: "cell-123".to_string(),
            },
        )
        .await;

        match hello.body {
            ExecResponse::ExecError { code, .. } => {
                assert_eq!(code, ExecErrorCode::Unauthorized);
            }
            other => panic!("unexpected response: {other:?}"),
        }

        server
            .await
            .expect("server should exit cleanly")
            .expect("worker run should succeed");
    }

    #[test]
    fn control_stream_from_fd_rejects_bad_fd() {
        let result = super::control_stream_from_fd(-1);
        assert!(result.is_err());

        let (_left, right) =
            std::os::unix::net::UnixStream::pair().expect("create unix stream pair");
        let raw = right.into_raw_fd();
        let _ = unsafe { std::os::unix::net::UnixStream::from_raw_fd(raw) };
    }
}
