use std::{collections::BTreeMap, path::PathBuf};

use planter_core::{CellId, CommandSpec, JobId};
use thiserror::Error;
use tokio::process::Child;

pub trait PlatformOps: Send + Sync {
    fn create_cell_dirs(&self, cell_id: &CellId) -> Result<CellPaths, PlatformError>;

    fn spawn_job(
        &self,
        job_id: &JobId,
        cell_id: &CellId,
        cmd: &CommandSpec,
        env: &BTreeMap<String, String>,
    ) -> Result<JobHandle, PlatformError>;

    fn kill_job_tree(&self, job_id: &JobId) -> Result<(), PlatformError>;

    fn probe_usage(&self, job_id: &JobId) -> Result<Option<JobUsage>, PlatformError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellPaths {
    pub cell_dir: PathBuf,
}

pub struct JobHandle {
    pub pid: Option<u32>,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
    pub child: Child,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobUsage {
    pub rss_bytes: Option<u64>,
    pub cpu_nanos: Option<u64>,
}

#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("unsupported operation: {0}")]
    Unsupported(String),
}
