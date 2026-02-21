use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};

use planter_core::{CellId, CommandSpec, JobId};
use planter_platform::{CellPaths, JobHandle, JobUsage, PlatformError, PlatformOps};
use tokio::process::{Child, Command};

#[derive(Debug, Clone)]
pub struct MacosOps {
    root: PathBuf,
}

impl MacosOps {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn cells_dir(&self) -> PathBuf {
        self.root.join("cells")
    }

    fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn compile_sandbox_profile(&self, _cell_id: &CellId) -> Result<PathBuf, PlatformError> {
        // TODO: generate and compile seatbelt profile for this cell.
        Err(PlatformError::Unsupported(
            "sandbox profile compilation is not implemented yet".to_string(),
        ))
    }

    pub fn lease_user(&self) -> Result<String, PlatformError> {
        // TODO: lease per-cell user from pool before sandboxed spawn.
        Err(PlatformError::Unsupported(
            "user pooling is not implemented yet".to_string(),
        ))
    }

    fn spawn_sandboxed(&self, command: &mut Command) -> Result<Child, PlatformError> {
        // TODO: replace with sandbox-exec/native sandbox APIs.
        command.spawn().map_err(PlatformError::from)
    }

    fn ensure_cell_exists(&self, cell_id: &CellId) -> Result<PathBuf, PlatformError> {
        let cell_dir = self.cells_dir().join(&cell_id.0);
        if !cell_dir.exists() {
            return Err(PlatformError::InvalidInput(format!(
                "cell {} does not exist",
                cell_id.0
            )));
        }
        Ok(cell_dir)
    }

    fn ensure_logs_dir(&self) -> Result<PathBuf, PlatformError> {
        let logs_dir = self.logs_dir();
        fs::create_dir_all(&logs_dir)?;
        Ok(logs_dir)
    }
}

impl PlatformOps for MacosOps {
    fn create_cell_dirs(&self, cell_id: &CellId) -> Result<CellPaths, PlatformError> {
        let cell_dir = self.cells_dir().join(&cell_id.0);
        fs::create_dir_all(&cell_dir)?;
        Ok(CellPaths { cell_dir })
    }

    fn spawn_job(
        &self,
        job_id: &JobId,
        cell_id: &CellId,
        cmd: &CommandSpec,
        env: &BTreeMap<String, String>,
    ) -> Result<JobHandle, PlatformError> {
        if cmd.argv.is_empty() {
            return Err(PlatformError::InvalidInput(
                "command argv cannot be empty".to_string(),
            ));
        }

        let cell_dir = self.ensure_cell_exists(cell_id)?;
        let logs_dir = self.ensure_logs_dir()?;

        let stdout_path = logs_dir.join(format!("{}.stdout.log", job_id.0));
        let stderr_path = logs_dir.join(format!("{}.stderr.log", job_id.0));

        let stdout_file = fs::File::create(&stdout_path)?;
        let stderr_file = fs::File::create(&stderr_path)?;

        let mut command = Command::new(&cmd.argv[0]);
        if cmd.argv.len() > 1 {
            command.args(&cmd.argv[1..]);
        }

        if let Some(cwd) = &cmd.cwd {
            command.current_dir(Path::new(cwd));
        } else {
            command.current_dir(cell_dir);
        }

        command.envs(env.clone());
        command.stdout(Stdio::from(stdout_file));
        command.stderr(Stdio::from(stderr_file));

        let child = self.spawn_sandboxed(&mut command)?;
        Ok(JobHandle {
            pid: child.id(),
            stdout_path,
            stderr_path,
            child,
        })
    }

    fn kill_job_tree(&self, _job_id: &JobId) -> Result<(), PlatformError> {
        // TODO: map job -> process group and terminate process tree.
        Err(PlatformError::Unsupported(
            "job termination is not implemented yet".to_string(),
        ))
    }

    fn probe_usage(&self, _job_id: &JobId) -> Result<Option<JobUsage>, PlatformError> {
        // TODO: gather RSS/CPU for running job via platform APIs.
        Ok(None)
    }
}
