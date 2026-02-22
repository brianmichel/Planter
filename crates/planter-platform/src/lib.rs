use std::{collections::BTreeMap, path::PathBuf};

use planter_core::{CellId, CommandSpec, JobId};
use thiserror::Error;
use tokio::process::Child;

/// Platform abstraction for filesystem/process operations used by workers.
pub trait PlatformOps: Send + Sync {
    /// Creates per-cell directories and returns resolved paths.
    fn create_cell_dirs(&self, cell_id: &CellId) -> Result<CellPaths, PlatformError>;

    /// Launches a job process for a cell with merged environment variables.
    fn spawn_job(
        &self,
        job_id: &JobId,
        cell_id: &CellId,
        cmd: &CommandSpec,
        env: &BTreeMap<String, String>,
    ) -> Result<JobHandle, PlatformError>;

    /// Terminates a job and any descendants.
    fn kill_job_tree(&self, job_id: &JobId, force: bool) -> Result<(), PlatformError>;

    /// Returns a point-in-time resource usage sample for a job, if available.
    fn probe_usage(&self, job_id: &JobId) -> Result<Option<JobUsage>, PlatformError>;
}

/// Paths created for a logical execution cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellPaths {
    /// Absolute path to the cell root directory.
    pub cell_dir: PathBuf,
}

/// Handle to a newly spawned job process and its log files.
pub struct JobHandle {
    /// Spawned process id when available.
    pub pid: Option<u32>,
    /// Stdout log path.
    pub stdout_path: PathBuf,
    /// Stderr log path.
    pub stderr_path: PathBuf,
    /// Tokio child handle for lifecycle management.
    pub child: Child,
}

/// Process resource metrics sampled by a platform backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobUsage {
    /// Resident set size in bytes.
    pub rss_bytes: Option<u64>,
    /// CPU nanoseconds used by the process.
    pub cpu_nanos: Option<u64>,
}

/// Errors emitted by platform backends.
#[derive(Debug, Error)]
pub enum PlatformError {
    /// Underlying I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Caller supplied invalid input.
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// Operation is not supported on this backend.
    #[error("unsupported operation: {0}")]
    Unsupported(String),
}
