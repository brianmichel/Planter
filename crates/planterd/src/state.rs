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
use planter_platform::{PlatformError, PlatformOps};
use tokio::{task, time::sleep};

use crate::pty::{PtyManager, PtyOpenResult, PtyReadResult, PtySandboxMode};

pub struct StateStore {
    root: PathBuf,
    id_counter: AtomicU64,
    platform: Arc<dyn PlatformOps>,
    pty: Arc<PtyManager>,
}

pub struct LogsReadResult {
    pub offset: u64,
    pub data: Vec<u8>,
    pub eof: bool,
    pub complete: bool,
}

pub struct JobKillResult {
    pub job: JobInfo,
    pub signal: String,
}

impl StateStore {
    pub fn new(
        root: PathBuf,
        platform: Arc<dyn PlatformOps>,
        pty_sandbox_mode: PtySandboxMode,
    ) -> Result<Self, PlanterError> {
        let store = Self {
            root: root.clone(),
            id_counter: AtomicU64::new(now_ms()),
            platform,
            pty: Arc::new(PtyManager::new(root, pty_sandbox_mode)),
        };
        store.ensure_layout()?;
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

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

    pub fn load_job(&self, job_id: &JobId) -> Result<JobInfo, PlanterError> {
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

        let handle = self
            .platform
            .spawn_job(&job_id, &cell_id, &cmd, &env)
            .map_err(platform_to_planter_error)?;

        let job = JobInfo {
            id: job_id.clone(),
            cell_id,
            command: cmd,
            stdout_path: handle.stdout_path.to_string_lossy().to_string(),
            stderr_path: handle.stderr_path.to_string_lossy().to_string(),
            started_at_ms: now_ms(),
            finished_at_ms: None,
            pid: handle.pid,
            status: ExitStatus::Running,
            termination_reason: None,
        };

        write_json(self.job_path(&job_id), &job)?;

        let root = self.root.clone();
        let job_id_for_task = job_id.clone();
        let mut child = handle.child;
        task::spawn(async move {
            match child.wait().await {
                Ok(status) => {
                    if let Err(err) = finalize_job_status(
                        &root,
                        &job_id_for_task,
                        ExitStatus::Exited {
                            code: status.code(),
                        },
                        Some(TerminationReason::Exited),
                    ) {
                        tracing::error!(error = %err, job_id = %job_id_for_task.0, "failed to persist finished job state");
                    }
                }
                Err(err) => {
                    tracing::error!(error = %err, job_id = %job_id_for_task.0, "failed while waiting for job process");
                }
            }
        });

        Ok(job)
    }

    pub fn kill_job(&self, job_id: &JobId, force: bool) -> Result<JobKillResult, PlanterError> {
        let mut job = self.load_job(job_id)?;
        if matches!(job.status, ExitStatus::Running) {
            self.platform
                .kill_job_tree(job_id, force)
                .map_err(platform_to_planter_error)?;
            job.status = ExitStatus::Exited { code: None };
            job.finished_at_ms = Some(now_ms());
            job.termination_reason = Some(if force {
                TerminationReason::ForcedKill
            } else {
                TerminationReason::TerminatedByUser
            });
            write_json(self.job_path(job_id), &job)?;
        }

        Ok(JobKillResult {
            job,
            signal: if force {
                "KILL".to_string()
            } else {
                "TERM".to_string()
            },
        })
    }

    pub fn remove_cell(&self, cell_id: &CellId, force: bool) -> Result<(), PlanterError> {
        let cell_meta = self.cell_meta_path(cell_id);
        if !cell_meta.exists() {
            return Err(PlanterError {
                code: ErrorCode::NotFound,
                message: format!("cell {} does not exist", cell_id.0),
                detail: None,
            });
        }

        let running_jobs: Vec<JobInfo> = self
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
            for mut job in running_jobs {
                self.platform
                    .kill_job_tree(&job.id, true)
                    .map_err(platform_to_planter_error)?;
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
            let job = self.load_job(job_id)?;
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

    pub fn open_pty(
        &self,
        shell: String,
        args: Vec<String>,
        cwd: Option<String>,
        env: BTreeMap<String, String>,
        cols: u16,
        rows: u16,
    ) -> Result<PtyOpenResult, PlanterError> {
        self.pty.open(shell, args, cwd, env, cols, rows)
    }

    pub fn pty_input(&self, session_id: SessionId, data: Vec<u8>) -> Result<(), PlanterError> {
        self.pty.input(session_id, data)
    }

    pub async fn pty_read(
        &self,
        session_id: SessionId,
        offset: u64,
        max_bytes: u32,
        follow: bool,
        wait_ms: u64,
    ) -> Result<PtyReadResult, PlanterError> {
        self.pty
            .read(session_id, offset, max_bytes, follow, wait_ms)
            .await
    }

    pub fn pty_resize(
        &self,
        session_id: SessionId,
        cols: u16,
        rows: u16,
    ) -> Result<(), PlanterError> {
        self.pty.resize(session_id, cols, rows)
    }

    pub fn pty_close(&self, session_id: SessionId, force: bool) -> Result<(), PlanterError> {
        self.pty.close(session_id, force)
    }

    fn jobs_for_cell(&self, cell_id: &CellId) -> Result<Vec<JobInfo>, PlanterError> {
        let mut jobs = Vec::new();
        let entries =
            fs::read_dir(self.jobs_dir()).map_err(|err| io_to_error("read jobs directory", err))?;

        for entry in entries {
            let entry = entry.map_err(|err| io_to_error("read jobs directory entry", err))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }

            let job: JobInfo = read_json(path)?;
            if job.cell_id == *cell_id {
                jobs.push(job);
            }
        }

        Ok(jobs)
    }

    fn ensure_layout(&self) -> Result<(), PlanterError> {
        fs::create_dir_all(self.cells_dir())
            .map_err(|err| io_to_error("create cells directory", err))?;
        fs::create_dir_all(self.jobs_dir())
            .map_err(|err| io_to_error("create jobs directory", err))?;
        fs::create_dir_all(self.logs_dir())
            .map_err(|err| io_to_error("create logs directory", err))?;
        Ok(())
    }

    fn next_id(&self) -> u64 {
        self.id_counter.fetch_add(1, Ordering::Relaxed)
    }

    fn cells_dir(&self) -> PathBuf {
        self.root.join("cells")
    }

    fn jobs_dir(&self) -> PathBuf {
        self.root.join("jobs")
    }

    fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    fn cell_meta_path(&self, cell_id: &CellId) -> PathBuf {
        self.cells_dir().join(&cell_id.0).join("cell.json")
    }

    fn job_path(&self, job_id: &JobId) -> PathBuf {
        self.jobs_dir().join(format!("{}.json", job_id.0))
    }
}

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

fn finalize_job_status(
    root: &Path,
    job_id: &JobId,
    status: ExitStatus,
    reason: Option<TerminationReason>,
) -> Result<(), PlanterError> {
    let path = root.join("jobs").join(format!("{}.json", job_id.0));
    let mut job: JobInfo = read_json(path.clone())?;
    job.finished_at_ms = Some(now_ms());
    job.status = status;
    if job.termination_reason.is_none() {
        job.termination_reason = reason;
    }
    write_json(path, &job)
}

fn write_json<T: serde::Serialize>(path: PathBuf, value: &T) -> Result<(), PlanterError> {
    let json = serde_json::to_vec_pretty(value).map_err(|err| PlanterError {
        code: ErrorCode::Internal,
        message: "serialize json".to_string(),
        detail: Some(err.to_string()),
    })?;

    fs::write(path, json).map_err(|err| io_to_error("write json file", err))
}

fn read_json<T: serde::de::DeserializeOwned>(path: PathBuf) -> Result<T, PlanterError> {
    let bytes = fs::read(path).map_err(|err| io_to_error("read json file", err))?;
    serde_json::from_slice(&bytes).map_err(|err| PlanterError {
        code: ErrorCode::Internal,
        message: "decode json".to_string(),
        detail: Some(err.to_string()),
    })
}

fn io_to_error(action: &str, err: io::Error) -> PlanterError {
    PlanterError {
        code: ErrorCode::Internal,
        message: action.to_string(),
        detail: Some(err.to_string()),
    }
}

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
