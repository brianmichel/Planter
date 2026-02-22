use std::sync::Arc;

use planter_core::{PROTOCOL_VERSION, PlanterError, PtyAction, Request, Response};

use crate::state::StateStore;

#[derive(Clone)]
pub struct Handler {
    state: Arc<StateStore>,
}

impl Handler {
    pub fn new(state: Arc<StateStore>) -> Self {
        Self { state }
    }

    pub async fn handle(&self, request: Request) -> Response {
        let result = match request {
            Request::Version {} => Ok(Response::Version {
                daemon: env!("CARGO_PKG_VERSION").to_string(),
                protocol: PROTOCOL_VERSION,
            }),
            Request::Health {} => Ok(Response::Health {
                status: "ok".to_string(),
            }),
            Request::CellCreate { spec } => self
                .state
                .create_cell(spec)
                .map(|cell| Response::CellCreated { cell }),
            Request::JobRun { cell_id, cmd } => self
                .state
                .run_job(cell_id, cmd)
                .await
                .map(|job| Response::JobStarted { job }),
            Request::JobStatus { job_id } => self
                .state
                .load_job(&job_id)
                .map(|job| Response::JobStatus { job }),
            Request::JobKill { job_id, force } => {
                self.state
                    .kill_job(&job_id, force)
                    .await
                    .map(|result| Response::JobKilled {
                        job_id,
                        signal: result.signal,
                        status: result.job.status,
                    })
            }
            Request::CellRemove { cell_id, force } => self
                .state
                .remove_cell(&cell_id, force)
                .map(|()| Response::CellRemoved { cell_id }),
            Request::LogsRead {
                job_id,
                stream,
                offset,
                max_bytes,
                follow,
                wait_ms,
            } => self
                .state
                .read_logs(&job_id, stream, offset, max_bytes, follow, wait_ms)
                .await
                .map(|chunk| Response::LogsChunk {
                    job_id,
                    stream,
                    offset: chunk.offset,
                    data: chunk.data,
                    eof: chunk.eof,
                    complete: chunk.complete,
                }),
            Request::PtyOpen {
                shell,
                args,
                cwd,
                env,
                cols,
                rows,
            } => self
                .state
                .open_pty(shell, args, cwd, env, cols, rows)
                .await
                .map(|opened| Response::PtyOpened {
                    session_id: opened.session_id,
                    pid: opened.pid,
                }),
            Request::PtyInput { session_id, data } => self
                .state
                .pty_input(session_id, data)
                .await
                .map(|()| Response::PtyAck {
                    session_id,
                    action: PtyAction::Input,
                }),
            Request::PtyRead {
                session_id,
                offset,
                max_bytes,
                follow,
                wait_ms,
            } => self
                .state
                .pty_read(session_id, offset, max_bytes, follow, wait_ms)
                .await
                .map(|chunk| Response::PtyChunk {
                    session_id,
                    offset: chunk.offset,
                    data: chunk.data,
                    eof: chunk.eof,
                    complete: chunk.complete,
                    exit_code: chunk.exit_code,
                }),
            Request::PtyResize {
                session_id,
                cols,
                rows,
            } => self
                .state
                .pty_resize(session_id, cols, rows)
                .await
                .map(|()| Response::PtyAck {
                    session_id,
                    action: PtyAction::Resize,
                }),
            Request::PtyClose { session_id, force } => self
                .state
                .pty_close(session_id, force)
                .await
                .map(|()| Response::PtyAck {
                    session_id,
                    action: PtyAction::Closed,
                }),
        };

        match result {
            Ok(response) => response,
            Err(err) => to_error_response(err),
        }
    }
}

fn to_error_response(err: PlanterError) -> Response {
    Response::Error {
        code: err.code,
        message: err.message,
        detail: err.detail,
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use std::{collections::BTreeMap, sync::Arc, time::Duration};

    use super::Handler;
    use planter_core::{CellSpec, CommandSpec, ErrorCode, JobId, LogStream, Request, Response};
    use planter_platform_macos::{MacosOps, SandboxMode};
    use tempfile::tempdir;
    use tokio::time::sleep;

    use crate::state::StateStore;

    fn test_handler(state_root: std::path::PathBuf) -> Handler {
        let platform = Arc::new(MacosOps::new(state_root.clone(), SandboxMode::Disabled));
        let state =
            Arc::new(StateStore::new(state_root, platform).expect("state should initialize"));
        Handler::new(state)
    }

    #[tokio::test]
    async fn lifecycle_and_logs_flow() {
        let tmp = tempdir().expect("tempdir");
        let state_root = tmp.path().join("state");
        let handler = test_handler(state_root);

        let created = handler
            .handle(Request::CellCreate {
                spec: CellSpec {
                    name: "demo".to_string(),
                    env: BTreeMap::new(),
                },
            })
            .await;
        let cell_id = match created {
            Response::CellCreated { cell } => cell.id,
            other => panic!("unexpected response: {other:?}"),
        };

        let started = handler
            .handle(Request::JobRun {
                cell_id: cell_id.clone(),
                cmd: CommandSpec {
                    argv: vec![
                        "/bin/sh".to_string(),
                        "-c".to_string(),
                        "echo hello-from-job; echo err-line >&2".to_string(),
                    ],
                    cwd: None,
                    env: BTreeMap::new(),
                    limits: None,
                },
            })
            .await;
        let job_id = match started {
            Response::JobStarted { job } => job.id,
            other => panic!("unexpected response: {other:?}"),
        };

        let mut saw_hello = false;
        for _ in 0..20 {
            let chunk = handler
                .handle(Request::LogsRead {
                    job_id: job_id.clone(),
                    stream: LogStream::Stdout,
                    offset: 0,
                    max_bytes: 4096,
                    follow: true,
                    wait_ms: 100,
                })
                .await;

            if let Response::LogsChunk { data, .. } = chunk
                && String::from_utf8_lossy(&data).contains("hello-from-job")
            {
                saw_hello = true;
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
        assert!(saw_hello, "expected stdout logs via LogsRead");

        let status = handler
            .handle(Request::JobStatus {
                job_id: job_id.clone(),
            })
            .await;
        match status {
            Response::JobStatus { job } => {
                assert_eq!(job.id, job_id);
            }
            other => panic!("unexpected response: {other:?}"),
        }

        let kill = handler
            .handle(Request::JobKill {
                job_id: job_id.clone(),
                force: true,
            })
            .await;
        match kill {
            Response::JobKilled { job_id: id, .. } => assert_eq!(id, job_id),
            other => panic!("unexpected response: {other:?}"),
        }

        let removed = handler
            .handle(Request::CellRemove {
                cell_id: cell_id.clone(),
                force: true,
            })
            .await;
        match removed {
            Response::CellRemoved { cell_id: id } => assert_eq!(id, cell_id),
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[tokio::test]
    async fn remove_cell_without_force_fails_when_job_running() {
        let tmp = tempdir().expect("tempdir");
        let state_root = tmp.path().join("state");
        let handler = test_handler(state_root);

        let created = handler
            .handle(Request::CellCreate {
                spec: CellSpec {
                    name: "demo".to_string(),
                    env: BTreeMap::new(),
                },
            })
            .await;
        let cell_id = match created {
            Response::CellCreated { cell } => cell.id,
            other => panic!("unexpected response: {other:?}"),
        };

        let started = handler
            .handle(Request::JobRun {
                cell_id: cell_id.clone(),
                cmd: CommandSpec {
                    argv: vec![
                        "/bin/sh".to_string(),
                        "-c".to_string(),
                        "sleep 10".to_string(),
                    ],
                    cwd: None,
                    env: BTreeMap::new(),
                    limits: None,
                },
            })
            .await;
        let job_id = match started {
            Response::JobStarted { job } => job.id,
            other => panic!("unexpected response: {other:?}"),
        };

        let removed = handler
            .handle(Request::CellRemove {
                cell_id,
                force: false,
            })
            .await;
        match removed {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::InvalidRequest),
            other => panic!("unexpected response: {other:?}"),
        }

        let _ = handler
            .handle(Request::JobKill {
                job_id: JobId(job_id.0),
                force: true,
            })
            .await;
    }
}
