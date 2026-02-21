use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use planter_core::{
    CellId, CellInfo, CellSpec, CommandSpec, ErrorCode, ExitStatus, JobId, JobInfo, PlanterError,
    now_ms,
};
use planter_platform::{PlatformError, PlatformOps};
use tokio::task;

pub struct StateStore {
    root: PathBuf,
    id_counter: AtomicU64,
    platform: Arc<dyn PlatformOps>,
}

impl StateStore {
    pub fn new(root: PathBuf, platform: Arc<dyn PlatformOps>) -> Result<Self, PlanterError> {
        let store = Self {
            root,
            id_counter: AtomicU64::new(now_ms()),
            platform,
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
        read_json(path).map_err(|err| match err.code {
            ErrorCode::Internal if err.message == "read json file" => {
                if let Some(detail) = &err.detail && detail.contains("No such file") {
                    return PlanterError {
                        code: ErrorCode::NotFound,
                        message: format!("cell {} does not exist", cell_id.0),
                        detail: None,
                    };
                }
                err
            }
            _ => err,
        })
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

fn finalize_job_status(
    root: &Path,
    job_id: &JobId,
    status: ExitStatus,
) -> Result<(), PlanterError> {
    let path = root.join("jobs").join(format!("{}.json", job_id.0));
    let mut job: JobInfo = read_json(path.clone())?;
    job.finished_at_ms = Some(now_ms());
    job.status = status;
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
