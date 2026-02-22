use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use planter_core::{
    CellId, CellInfo, CellSpec, CommandSpec, ErrorCode, ExitStatus, JobId, JobInfo, LogStream,
    PlanterError, SessionId, TerminationReason, now_ms,
};
use planter_execd_proto::{ExecPtyAction, ExecRequest, ExecResponse};
use planter_platform::{PlatformError, PlatformOps};
use tokio::time::sleep;

use crate::worker_manager::WorkerManager;

/// Persistent daemon state and orchestration entrypoint for jobs/PTYs.
pub struct StateStore {
    /// Root state directory.
    root: PathBuf,
    /// Monotonic id counter used for generated ids.
    id_counter: AtomicU64,
    /// Platform backend for filesystem/process operations.
    platform: Arc<dyn PlatformOps>,
    /// Worker lifecycle manager.
    workers: Arc<WorkerManager>,
}

/// Result payload for log read operations.
pub struct LogsReadResult {
    /// Requested offset.
    pub offset: u64,
    /// Returned log bytes.
    pub data: Vec<u8>,
    /// True when no more bytes are currently available.
    pub eof: bool,
    /// True when stream is complete and closed.
    pub complete: bool,
}

/// Result payload for job kill operations.
pub struct JobKillResult {
    /// Updated job metadata.
    pub job: JobInfo,
    /// Signal name applied to the process.
    pub signal: String,
}

/// Result payload for PTY open operations.
pub struct PtyOpenResult {
    /// Created session id.
    pub session_id: SessionId,
    /// Spawned shell pid when known.
    pub pid: Option<u32>,
}

/// Result payload for PTY read operations.
pub struct PtyReadResult {
    /// Requested offset.
    pub offset: u64,
    /// Returned PTY bytes.
    pub data: Vec<u8>,
    /// True when no more bytes are currently available.
    pub eof: bool,
    /// True when the PTY has exited and stream is complete.
    pub complete: bool,
    /// Shell exit code when complete.
    pub exit_code: Option<i32>,
}

/// Internal persisted job metadata representation on disk.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredJobInfo {
    /// Job identifier.
    id: JobId,
    /// Owning cell identifier.
    cell_id: CellId,
    /// Command specification.
    command: CommandSpec,
    /// Stdout log file path.
    stdout_path: String,
    /// Stderr log file path.
    stderr_path: String,
    /// Job start timestamp.
    started_at_ms: u64,
    /// Job finish timestamp if complete.
    finished_at_ms: Option<u64>,
    /// Child process id if known.
    pid: Option<u32>,
    /// Current exit status.
    status: ExitStatus,
    /// Optional termination cause.
    #[serde(default)]
    termination_reason: Option<TerminationReason>,
}

impl StoredJobInfo {
    /// Converts internal representation to public protocol job info.
    fn to_public(&self) -> JobInfo {
        JobInfo {
            id: self.id.clone(),
            cell_id: self.cell_id.clone(),
            command: self.command.clone(),
            started_at_ms: self.started_at_ms,
            finished_at_ms: self.finished_at_ms,
            pid: self.pid,
            status: self.status.clone(),
            termination_reason: self.termination_reason,
        }
    }
}

impl StateStore {
    /// Creates a new state store and ensures required directory layout exists.
    pub fn new(root: PathBuf, platform: Arc<dyn PlatformOps>) -> Result<Self, PlanterError> {
        let store = Self {
            root: root.clone(),
            id_counter: AtomicU64::new(now_ms()),
            platform,
            workers: Arc::new(WorkerManager::new(root.clone())),
        };
        store.ensure_layout()?;
        Ok(store)
    }

    /// Returns the configured root state directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Creates a new cell and persists its metadata.
    pub fn create_cell(&self, spec: CellSpec) -> Result<CellInfo, PlanterError> {
        if spec.name.trim().is_empty() {
            return Err(PlanterError {
                code: ErrorCode::InvalidRequest,
                message: "cell name cannot be empty".to_string(),
                detail: None,
            });
        }

        let cell_id = CellId(format!("cell-{}", self.next_id()));
        let created_at_ms = now_ms();
        let paths = self
            .platform
            .create_cell_dirs(&cell_id)
            .map_err(platform_to_planter_error)?;

        let info = CellInfo {
            id: cell_id,
            spec,
            created_at_ms,
            dir: paths.cell_dir.to_string_lossy().to_string(),
        };

        write_json(self.cell_meta_path(&info.id), &info)?;
        Ok(info)
    }

    /// Loads a cell metadata file by id.
    pub fn load_cell(&self, cell_id: &CellId) -> Result<CellInfo, PlanterError> {
        let path = self.cell_meta_path(cell_id);
        if !path.exists() {
            return Err(PlanterError {
                code: ErrorCode::NotFound,
                message: format!("cell {} does not exist", cell_id.0),
                detail: None,
            });
        }

        read_json(path)
    }

    /// Loads job metadata by id.
    pub fn load_job(&self, job_id: &JobId) -> Result<JobInfo, PlanterError> {
        Ok(self.load_job_record(job_id)?.to_public())
    }

    /// Loads the internal persisted job representation by id.
    fn load_job_record(&self, job_id: &JobId) -> Result<StoredJobInfo, PlanterError> {
        let path = self.job_path(job_id);
        if !path.exists() {
            return Err(PlanterError {
                code: ErrorCode::NotFound,
                message: format!("job {} does not exist", job_id.0),
                detail: None,
            });
        }

        read_json(path)
    }

    /// Launches a job in a cell through the worker manager and persists metadata.
    pub async fn run_job(
        &self,
        cell_id: CellId,
        cmd: CommandSpec,
    ) -> Result<JobInfo, PlanterError> {
        let cell = self.load_cell(&cell_id)?;

        if cmd.argv.is_empty() {
            return Err(PlanterError {
                code: ErrorCode::InvalidRequest,
                message: "command argv cannot be empty".to_string(),
                detail: None,
            });
        }

        let job_id = JobId(format!("job-{}", self.next_id()));

        let mut env = BTreeMap::new();
        env.extend(cell.spec.env.clone());
        env.extend(cmd.env.clone());

        let stdout_path = self.logs_dir().join(format!("{}.stdout.log", job_id.0));
        let stderr_path = self.logs_dir().join(format!("{}.stderr.log", job_id.0));
        let response = self
            .workers
            .call(
                &cell_id,
                ExecRequest::RunJob {
                    job_id: job_id.clone(),
                    cmd: cmd.clone(),
                    env: env.clone(),
                    stdout_path: stdout_path.display().to_string(),
                    stderr_path: stderr_path.display().to_string(),
                },
            )
            .await?;
        let pid = match response {
            ExecResponse::JobStarted {
                job_id: started,
                pid,
            } if started == job_id => pid,
            other => return Err(unexpected_worker_response("run job", other)),
        };

        let job = StoredJobInfo {
            id: job_id.clone(),
            cell_id,
            command: cmd,
            stdout_path: stdout_path.display().to_string(),
            stderr_path: stderr_path.display().to_string(),
            started_at_ms: now_ms(),
            finished_at_ms: None,
            pid,
            status: ExitStatus::Running,
            termination_reason: None,
        };

        write_json(self.job_path(&job_id), &job)?;
        Ok(job.to_public())
    }

    /// Signals a running job and updates persisted metadata.
    pub async fn kill_job(
        &self,
        job_id: &JobId,
        force: bool,
    ) -> Result<JobKillResult, PlanterError> {
        let mut job = self.load_job_record(job_id)?;
        if matches!(job.status, ExitStatus::Running) {
            let response = self
                .workers
                .call(
                    &job.cell_id,
                    ExecRequest::JobSignal {
                        job_id: job_id.clone(),
                        force,
                    },
                )
                .await?;
            match response {
                ExecResponse::JobStatus {
                    job_id: returned,
                    status,
                    finished_at_ms,
                    termination_reason,
                } if returned == *job_id => {
                    job.status = status;
                    job.finished_at_ms = finished_at_ms.or(Some(now_ms()));
                    job.termination_reason = termination_reason.or(Some(if force {
                        TerminationReason::ForcedKill
                    } else {
                        TerminationReason::TerminatedByUser
                    }));
                }
                other => return Err(unexpected_worker_response("job signal", other)),
            }
            write_json(self.job_path(job_id), &job)?;
        }

        Ok(JobKillResult {
            job: job.to_public(),
            signal: if force {
                "KILL".to_string()
            } else {
                "TERM".to_string()
            },
        })
    }

    /// Removes a cell and optionally force-terminates running jobs.
    pub fn remove_cell(&self, cell_id: &CellId, force: bool) -> Result<(), PlanterError> {
        let cell_meta = self.cell_meta_path(cell_id);
        if !cell_meta.exists() {
            return Err(PlanterError {
                code: ErrorCode::NotFound,
                message: format!("cell {} does not exist", cell_id.0),
                detail: None,
            });
        }

        let running_jobs: Vec<StoredJobInfo> = self
            .jobs_for_cell(cell_id)?
            .into_iter()
            .filter(|job| matches!(job.status, ExitStatus::Running))
            .collect();

        if !running_jobs.is_empty() && !force {
            return Err(PlanterError {
                code: ErrorCode::InvalidRequest,
                message: format!("cell {} has running jobs; pass force to remove", cell_id.0),
                detail: None,
            });
        }

        if force {
            self.workers.stop_worker(cell_id, true)?;
            for mut job in running_jobs {
                job.status = ExitStatus::Exited { code: None };
                job.finished_at_ms = Some(now_ms());
                job.termination_reason = Some(TerminationReason::ForcedKill);
                write_json(self.job_path(&job.id), &job)?;
            }
        }

        let cell_dir = self.cells_dir().join(&cell_id.0);
        if cell_dir.exists() {
            fs::remove_dir_all(&cell_dir)
                .map_err(|err| io_to_error("remove cell directory", err))?;
        }

        Ok(())
    }

    /// Reads a chunk of job logs with optional follow behavior.
    pub async fn read_logs(
        &self,
        job_id: &JobId,
        stream: LogStream,
        offset: u64,
        max_bytes: u32,
        follow: bool,
        wait_ms: u64,
    ) -> Result<LogsReadResult, PlanterError> {
        let start = Instant::now();
        let max_bytes = usize::try_from(max_bytes.max(1)).unwrap_or(1024 * 64);

        loop {
            let job = self.load_job_record(job_id)?;
            let log_path = match stream {
                LogStream::Stdout => PathBuf::from(&job.stdout_path),
                LogStream::Stderr => PathBuf::from(&job.stderr_path),
            };

            let (data, file_len) = read_log_chunk(&log_path, offset, max_bytes)?;
            let job_running = matches!(job.status, ExitStatus::Running);
            let eof = offset.saturating_add(data.len() as u64) >= file_len;

            if !data.is_empty() {
                return Ok(LogsReadResult {
                    offset,
                    data,
                    eof,
                    complete: eof && !job_running,
                });
            }

            if !job_running {
                return Ok(LogsReadResult {
                    offset,
                    data: Vec::new(),
                    eof: true,
                    complete: true,
                });
            }

            if !follow {
                return Ok(LogsReadResult {
                    offset,
                    data: Vec::new(),
                    eof,
                    complete: false,
                });
            }

            if start.elapsed() >= Duration::from_millis(wait_ms.max(1)) {
                return Ok(LogsReadResult {
                    offset,
                    data: Vec::new(),
                    eof: true,
                    complete: false,
                });
            }

            sleep(Duration::from_millis(75)).await;
        }
    }

    /// Opens a new PTY session via the PTY worker channel.
    pub async fn open_pty(
        &self,
        shell: String,
        args: Vec<String>,
        cwd: Option<String>,
        env: BTreeMap<String, String>,
        cols: u16,
        rows: u16,
    ) -> Result<PtyOpenResult, PlanterError> {
        let response = self
            .workers
            .call(
                &default_pty_cell_id(),
                ExecRequest::PtyOpen {
                    shell,
                    args,
                    cwd,
                    env,
                    cols,
                    rows,
                },
            )
            .await?;
        match response {
            ExecResponse::PtyOpened { session_id, pid } => Ok(PtyOpenResult { session_id, pid }),
            other => Err(unexpected_worker_response("pty open", other)),
        }
    }

    /// Sends input bytes to an existing PTY session.
    pub async fn pty_input(
        &self,
        session_id: SessionId,
        data: Vec<u8>,
    ) -> Result<(), PlanterError> {
        let response = self
            .workers
            .call(
                &default_pty_cell_id(),
                ExecRequest::PtyInput { session_id, data },
            )
            .await?;
        match response {
            ExecResponse::PtyAck {
                session_id: ack_id,
                action: ExecPtyAction::Input,
            } if ack_id == session_id => Ok(()),
            other => Err(unexpected_worker_response("pty input", other)),
        }
    }

    /// Reads output bytes from an existing PTY session.
    pub async fn pty_read(
        &self,
        session_id: SessionId,
        offset: u64,
        max_bytes: u32,
        follow: bool,
        wait_ms: u64,
    ) -> Result<PtyReadResult, PlanterError> {
        let response = self
            .workers
            .call(
                &default_pty_cell_id(),
                ExecRequest::PtyRead {
                    session_id,
                    offset,
                    max_bytes,
                    follow,
                    wait_ms,
                },
            )
            .await?;
        match response {
            ExecResponse::PtyChunk {
                session_id: chunk_id,
                offset,
                data,
                eof,
                complete,
                exit_code,
            } if chunk_id == session_id => Ok(PtyReadResult {
                offset,
                data,
                eof,
                complete,
                exit_code,
            }),
            other => Err(unexpected_worker_response("pty read", other)),
        }
    }

    /// Resizes an existing PTY session.
    pub async fn pty_resize(
        &self,
        session_id: SessionId,
        cols: u16,
        rows: u16,
    ) -> Result<(), PlanterError> {
        let response = self
            .workers
            .call(
                &default_pty_cell_id(),
                ExecRequest::PtyResize {
                    session_id,
                    cols,
                    rows,
                },
            )
            .await?;
        match response {
            ExecResponse::PtyAck {
                session_id: ack_id,
                action: ExecPtyAction::Resize,
            } if ack_id == session_id => Ok(()),
            other => Err(unexpected_worker_response("pty resize", other)),
        }
    }

    /// Closes an existing PTY session.
    pub async fn pty_close(&self, session_id: SessionId, force: bool) -> Result<(), PlanterError> {
        let response = self
            .workers
            .call(
                &default_pty_cell_id(),
                ExecRequest::PtyClose { session_id, force },
            )
            .await?;
        match response {
            ExecResponse::PtyAck {
                session_id: ack_id,
                action: ExecPtyAction::Closed,
            } if ack_id == session_id => Ok(()),
            other => Err(unexpected_worker_response("pty close", other)),
        }
    }

    /// Returns all jobs currently associated with a cell.
    fn jobs_for_cell(&self, cell_id: &CellId) -> Result<Vec<StoredJobInfo>, PlanterError> {
        let mut jobs = Vec::new();
        let entries =
            fs::read_dir(self.jobs_dir()).map_err(|err| io_to_error("read jobs directory", err))?;

        for entry in entries {
            let entry = entry.map_err(|err| io_to_error("read jobs directory entry", err))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }

            let job: StoredJobInfo = read_json(path)?;
            if job.cell_id == *cell_id {
                jobs.push(job);
            }
        }

        Ok(jobs)
    }

    /// Ensures required state directories exist.
    fn ensure_layout(&self) -> Result<(), PlanterError> {
        fs::create_dir_all(self.cells_dir())
            .map_err(|err| io_to_error("create cells directory", err))?;
        fs::create_dir_all(self.jobs_dir())
            .map_err(|err| io_to_error("create jobs directory", err))?;
        fs::create_dir_all(self.logs_dir())
            .map_err(|err| io_to_error("create logs directory", err))?;
        Ok(())
    }

    /// Returns the next monotonic id value.
    fn next_id(&self) -> u64 {
        self.id_counter.fetch_add(1, Ordering::Relaxed)
    }

    /// Returns the cells directory path.
    fn cells_dir(&self) -> PathBuf {
        self.root.join("cells")
    }

    /// Returns the jobs metadata directory path.
    fn jobs_dir(&self) -> PathBuf {
        self.root.join("jobs")
    }

    /// Returns the logs directory path.
    fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// Returns the metadata file path for a cell.
    fn cell_meta_path(&self, cell_id: &CellId) -> PathBuf {
        self.cells_dir().join(&cell_id.0).join("cell.json")
    }

    /// Returns the metadata file path for a job.
    fn job_path(&self, job_id: &JobId) -> PathBuf {
        self.jobs_dir().join(format!("{}.json", job_id.0))
    }
}

/// Reads a slice of bytes from a log file using offset and max byte count.
fn read_log_chunk(
    path: &Path,
    offset: u64,
    max_bytes: usize,
) -> Result<(Vec<u8>, u64), PlanterError> {
    match fs::read(path) {
        Ok(bytes) => {
            let file_len = bytes.len() as u64;
            let start = usize::try_from(offset).unwrap_or(bytes.len());
            if start >= bytes.len() {
                return Ok((Vec::new(), file_len));
            }
            let end = start.saturating_add(max_bytes).min(bytes.len());
            Ok((bytes[start..end].to_vec(), file_len))
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok((Vec::new(), 0)),
        Err(err) => Err(io_to_error("read log file", err)),
    }
}

/// Serializes a value as pretty JSON to disk.
fn write_json<T: serde::Serialize>(path: PathBuf, value: &T) -> Result<(), PlanterError> {
    let json = serde_json::to_vec_pretty(value).map_err(|err| PlanterError {
        code: ErrorCode::Internal,
        message: "serialize json".to_string(),
        detail: Some(err.to_string()),
    })?;

    fs::write(path, json).map_err(|err| io_to_error("write json file", err))
}

/// Reads and decodes a JSON file into a typed value.
fn read_json<T: serde::de::DeserializeOwned>(path: PathBuf) -> Result<T, PlanterError> {
    let bytes = fs::read(path).map_err(|err| io_to_error("read json file", err))?;
    serde_json::from_slice(&bytes).map_err(|err| PlanterError {
        code: ErrorCode::Internal,
        message: "decode json".to_string(),
        detail: Some(err.to_string()),
    })
}

/// Converts plain I/O errors to standardized planter errors.
fn io_to_error(action: &str, err: io::Error) -> PlanterError {
    PlanterError {
        code: ErrorCode::Internal,
        message: action.to_string(),
        detail: Some(err.to_string()),
    }
}

/// Maps platform backend errors into daemon protocol errors.
fn platform_to_planter_error(err: PlatformError) -> PlanterError {
    match err {
        PlatformError::Io(io_err) => io_to_error("platform io", io_err),
        PlatformError::InvalidInput(message) => PlanterError {
            code: ErrorCode::InvalidRequest,
            message,
            detail: None,
        },
        PlatformError::Unsupported(message) => PlanterError {
            code: ErrorCode::Internal,
            message: "platform unsupported".to_string(),
            detail: Some(message),
        },
    }
}

/// Returns the dedicated logical cell id used for PTY worker calls.
fn default_pty_cell_id() -> CellId {
    CellId("cell-pty-default".to_string())
}

/// Builds a standardized error for unexpected worker response variants.
fn unexpected_worker_response(action: &str, response: ExecResponse) -> PlanterError {
    PlanterError {
        code: ErrorCode::Internal,
        message: format!("unexpected worker response for {action}"),
        detail: Some(format!("{response:?}")),
    }
}
