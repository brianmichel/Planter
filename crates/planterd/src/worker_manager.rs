#![allow(dead_code)]

use std::{
    collections::HashMap,
    os::fd::AsRawFd,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use planter_core::{CellId, ErrorCode, PlanterError, now_ms};
use planter_execd::WorkerConfig;
use planter_execd_proto::{ExecRequest, ExecResponse};
use tokio::{
    net::UnixStream,
    process::{Child, Command},
    sync::Mutex as AsyncMutex,
    task::JoinHandle,
    time::timeout,
};

use crate::worker::{WorkerClient, new_auth_token};

/// Default path used when no explicit worker binary override is provided.
const DEFAULT_WORKER_BIN: &str = "target/debug/planter-execd";
/// Maximum handshake wait before considering worker startup failed.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(2_000);
/// Per-cell async mutex used to serialize calls into a worker.
type CallLock = Arc<AsyncMutex<()>>;
/// Mapping from cell id to call lock.
type CallLockMap = HashMap<String, CallLock>;

/// Lifecycle manager for `planter-execd` worker processes.
pub struct WorkerManager {
    /// Worker executable path.
    worker_bin: PathBuf,
    /// Root state directory passed to workers.
    state_root: PathBuf,
    /// Active workers keyed by cell id.
    workers: Mutex<HashMap<String, WorkerHandle>>,
    /// Per-cell request serialization locks.
    call_locks: Mutex<CallLockMap>,
}

/// In-memory handle for one active worker.
struct WorkerHandle {
    /// RPC client to the worker control socket.
    client: WorkerClient,
    /// Runtime ownership for process or in-process task.
    runtime: WorkerRuntime,
    /// Last successful request timestamp in milliseconds.
    last_used_ms: u64,
}

/// Worker execution model used by the manager.
enum WorkerRuntime {
    /// Dedicated OS process.
    Process(Child),
    /// In-process tokio task used when binary is unavailable.
    InProcess(JoinHandle<Result<(), planter_execd::WorkerError>>),
}

impl WorkerHandle {
    /// Attempts graceful worker shutdown, then forcefully tears down runtime.
    async fn terminate(&mut self) {
        let _ = self
            .client
            .call(ExecRequest::Shutdown { force: true })
            .await;
        match &mut self.runtime {
            WorkerRuntime::Process(child) => {
                let _ = child.kill().await;
            }
            WorkerRuntime::InProcess(task) => {
                task.abort();
            }
        }
    }
}

impl WorkerManager {
    /// Creates a worker manager using environment/default worker binary path.
    pub fn new(state_root: PathBuf) -> Self {
        Self {
            worker_bin: std::env::var("PLANTER_EXECD_BIN")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(DEFAULT_WORKER_BIN)),
            state_root,
            workers: Mutex::new(HashMap::new()),
            call_locks: Mutex::new(HashMap::new()),
        }
    }

    /// Creates a worker manager with an explicit worker binary path.
    pub fn with_worker_bin(state_root: PathBuf, worker_bin: PathBuf) -> Self {
        Self {
            worker_bin,
            state_root,
            workers: Mutex::new(HashMap::new()),
            call_locks: Mutex::new(HashMap::new()),
        }
    }

    /// Sends one request to the worker for the given cell, spawning as needed.
    pub async fn call(
        &self,
        cell_id: &CellId,
        request: ExecRequest,
    ) -> Result<ExecResponse, PlanterError> {
        let key = cell_id.0.clone();
        let call_lock = self.get_call_lock(&key)?;
        let _call_guard = call_lock.lock().await;

        let mut handle = match self.take_worker(&key)? {
            Some(mut existing) => {
                if existing.client.ping().await.is_ok() {
                    existing
                } else {
                    existing.terminate().await;
                    self.spawn_worker(cell_id).await?
                }
            }
            None => self.spawn_worker(cell_id).await?,
        };

        let response = handle.client.call(request).await;
        match response {
            Ok(response) => {
                handle.last_used_ms = now_ms();
                self.put_worker(key, handle)?;
                Ok(response)
            }
            Err(err) => {
                handle.terminate().await;
                Err(err)
            }
        }
    }

    /// Stops and forgets the worker associated with a cell id.
    pub fn stop_worker(&self, cell_id: &CellId, _force: bool) -> Result<(), PlanterError> {
        let key = cell_id.0.clone();
        let Some(mut handle) = self.take_worker(&key)? else {
            return Ok(());
        };

        match &mut handle.runtime {
            WorkerRuntime::Process(child) => {
                let _ = child.start_kill();
            }
            WorkerRuntime::InProcess(task) => task.abort(),
        }
        let _ = self.call_locks_lock()?.remove(&key);
        Ok(())
    }

    /// Spawns or boots a worker runtime and completes hello handshake.
    async fn spawn_worker(&self, cell_id: &CellId) -> Result<WorkerHandle, PlanterError> {
        let (parent_std, child_std) =
            std::os::unix::net::UnixStream::pair().map_err(|err| PlanterError {
                code: ErrorCode::Unavailable,
                message: "create worker socketpair".to_string(),
                detail: Some(err.to_string()),
            })?;
        parent_std
            .set_nonblocking(true)
            .map_err(|err| PlanterError {
                code: ErrorCode::Unavailable,
                message: "configure worker socketpair".to_string(),
                detail: Some(err.to_string()),
            })?;
        child_std
            .set_nonblocking(true)
            .map_err(|err| PlanterError {
                code: ErrorCode::Unavailable,
                message: "configure worker socketpair".to_string(),
                detail: Some(err.to_string()),
            })?;

        let child_fd = child_std.as_raw_fd();
        let parent_stream = UnixStream::from_std(parent_std).map_err(|err| PlanterError {
            code: ErrorCode::Unavailable,
            message: "convert worker socket".to_string(),
            detail: Some(err.to_string()),
        })?;

        let auth_token = new_auth_token();
        let runtime = if use_inprocess_worker(&self.worker_bin) {
            let child_stream = UnixStream::from_std(child_std).map_err(|err| PlanterError {
                code: ErrorCode::Unavailable,
                message: "convert in-process worker socket".to_string(),
                detail: Some(err.to_string()),
            })?;
            let config = WorkerConfig {
                cell_id: cell_id.0.clone(),
                auth_token: auth_token.clone(),
                state_root: self.state_root.clone(),
            };
            let task = tokio::spawn(async move {
                planter_execd::serve_control_stream(child_stream, config).await
            });
            WorkerRuntime::InProcess(task)
        } else {
            clear_close_on_exec(child_fd)?;
            let mut command = Command::new(&self.worker_bin);
            command
                .arg("--control-fd")
                .arg(child_fd.to_string())
                .arg("--auth-token")
                .arg(&auth_token)
                .arg("--cell-id")
                .arg(&cell_id.0)
                .arg("--state-root")
                .arg(self.state_root.display().to_string());

            let child = command.spawn().map_err(|err| PlanterError {
                code: ErrorCode::Unavailable,
                message: "spawn planter-execd".to_string(),
                detail: Some(format!("{}: {err}", self.worker_bin.display())),
            })?;
            drop(child_std);
            WorkerRuntime::Process(child)
        };

        let mut client = WorkerClient::new(parent_stream);
        let hello = timeout(
            HANDSHAKE_TIMEOUT,
            client.hello(auth_token, cell_id.0.clone()),
        )
        .await;
        match hello {
            Ok(Ok(())) => Ok(WorkerHandle {
                client,
                runtime,
                last_used_ms: now_ms(),
            }),
            Ok(Err(err)) => {
                let mut handle = WorkerHandle {
                    client,
                    runtime,
                    last_used_ms: now_ms(),
                };
                handle.terminate().await;
                Err(err)
            }
            Err(_) => {
                let mut handle = WorkerHandle {
                    client,
                    runtime,
                    last_used_ms: now_ms(),
                };
                handle.terminate().await;
                Err(PlanterError {
                    code: ErrorCode::Unavailable,
                    message: "worker hello timed out".to_string(),
                    detail: Some(format!("timeout_ms={}", HANDSHAKE_TIMEOUT.as_millis())),
                })
            }
        }
    }

    /// Removes and returns a cached worker handle for a key.
    fn take_worker(&self, key: &str) -> Result<Option<WorkerHandle>, PlanterError> {
        Ok(self.workers_lock()?.remove(key))
    }

    /// Stores a worker handle for a key.
    fn put_worker(&self, key: String, worker: WorkerHandle) -> Result<(), PlanterError> {
        self.workers_lock()?.insert(key, worker);
        Ok(())
    }

    /// Acquires the worker map lock and converts poisoning to planter errors.
    fn workers_lock(&self) -> Result<MutexGuard<'_, HashMap<String, WorkerHandle>>, PlanterError> {
        self.workers.lock().map_err(|_| PlanterError {
            code: ErrorCode::Internal,
            message: "worker manager lock poisoned".to_string(),
            detail: None,
        })
    }

    /// Returns the per-cell call lock, creating one if absent.
    fn get_call_lock(&self, key: &str) -> Result<CallLock, PlanterError> {
        let mut locks = self.call_locks_lock()?;
        if let Some(lock) = locks.get(key) {
            return Ok(Arc::clone(lock));
        }
        let lock = Arc::new(AsyncMutex::new(()));
        locks.insert(key.to_string(), Arc::clone(&lock));
        Ok(lock)
    }

    /// Acquires the call-lock map and converts poisoning to planter errors.
    fn call_locks_lock(&self) -> Result<MutexGuard<'_, CallLockMap>, PlanterError> {
        self.call_locks.lock().map_err(|_| PlanterError {
            code: ErrorCode::Internal,
            message: "worker manager call-lock map poisoned".to_string(),
            detail: None,
        })
    }
}

/// Selects in-process worker mode based on env override or binary presence.
fn use_inprocess_worker(worker_bin: &std::path::Path) -> bool {
    match std::env::var("PLANTER_EXECD_INPROC") {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => !worker_bin.exists(),
    }
}

/// Clears `FD_CLOEXEC` for an inherited fd passed to the worker process.
fn clear_close_on_exec(fd: i32) -> Result<(), PlanterError> {
    // SAFETY: fcntl is called with valid command constants and the provided fd.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(PlanterError {
            code: ErrorCode::Unavailable,
            message: "read worker fd flags".to_string(),
            detail: Some(std::io::Error::last_os_error().to_string()),
        });
    }
    // SAFETY: fcntl is called with valid command constants and the provided fd.
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) };
    if rc < 0 {
        return Err(PlanterError {
            code: ErrorCode::Unavailable,
            message: "clear worker fd close-on-exec".to_string(),
            detail: Some(std::io::Error::last_os_error().to_string()),
        });
    }
    Ok(())
}
