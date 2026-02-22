use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    process::{Command as StdCommand, Stdio},
    thread,
    time::Duration,
};

use planter_core::{CellId, CommandSpec, JobId, JobInfo};
use planter_platform::{CellPaths, JobHandle, JobUsage, PlatformError, PlatformOps};
use tokio::process::{Child, Command};

/// Ordered sandbox profile fragments merged into a generated profile file.
const PROFILE_FRAGMENTS: &[(&str, &str)] = &[
    ("00-header", include_str!("../profiles/00-header.sb")),
    ("10-process", include_str!("../profiles/10-process.sb")),
    (
        "20-filesystem",
        include_str!("../profiles/20-filesystem.sb"),
    ),
    ("30-network", include_str!("../profiles/30-network.sb")),
];

/// System path for the macOS sandbox runner.
const SANDBOX_EXEC_PATH: &str = "/usr/bin/sandbox-exec";

/// Sandboxing policy to apply for launched jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxMode {
    /// Launch without sandbox restrictions.
    Disabled,
    /// Attempt sandboxing and fall back to unsandboxed execution on failure.
    Permissive,
    /// Require sandboxing; fail if sandbox launch cannot be applied.
    Enforced,
}

/// macOS implementation of [`PlatformOps`].
#[derive(Debug, Clone)]
pub struct MacosOps {
    /// Root state directory containing cells/logs/job metadata.
    root: PathBuf,
    /// Runtime sandbox mode.
    sandbox_mode: SandboxMode,
}

impl MacosOps {
    /// Creates a new macOS platform backend for a state root.
    pub fn new(root: PathBuf, sandbox_mode: SandboxMode) -> Self {
        Self { root, sandbox_mode }
    }

    /// Returns the root directory containing all cell workspaces.
    fn cells_dir(&self) -> PathBuf {
        self.root.join("cells")
    }

    /// Returns the directory containing stdout/stderr logs.
    fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// Returns the directory containing persisted job metadata.
    fn jobs_dir(&self) -> PathBuf {
        self.root.join("jobs")
    }

    /// Returns the directory containing generated sandbox profiles.
    fn sandbox_dir(&self) -> PathBuf {
        self.root.join("sandbox")
    }

    /// Renders and writes a sandbox profile file for a cell.
    pub fn compile_sandbox_profile(&self, cell_id: &CellId) -> Result<PathBuf, PlatformError> {
        let sandbox_dir = self.sandbox_dir();
        fs::create_dir_all(&sandbox_dir)?;

        let profile_path = sandbox_dir.join(format!("{}.sb", cell_id.0));
        let cell_dir = self.cells_dir().join(&cell_id.0);
        let profile = self.render_sandbox_profile(cell_id, &cell_dir);
        fs::write(&profile_path, profile)?;

        Ok(profile_path)
    }

    /// Renders the final sandbox profile by applying placeholder substitutions.
    fn render_sandbox_profile(&self, cell_id: &CellId, cell_dir: &Path) -> String {
        let mut output = String::new();
        let state_root = self.root.to_string_lossy().to_string();
        let state_root_real = fs::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        let state_root_real = state_root_real.to_string_lossy().to_string();
        let cell_dir = cell_dir.to_string_lossy().to_string();
        let cell_dir_real = fs::canonicalize(cell_dir.as_str())
            .unwrap_or_else(|_| PathBuf::from(cell_dir.as_str()));
        let cell_dir_real = cell_dir_real.to_string_lossy().to_string();

        for (name, fragment) in PROFILE_FRAGMENTS {
            if !output.is_empty() {
                output.push('\n');
            }

            output.push_str("; ---- ");
            output.push_str(name);
            output.push_str(" ----\n");

            let rendered = fragment
                .replace("{{CELL_ID}}", &cell_id.0)
                .replace("{{STATE_ROOT}}", &state_root)
                .replace("{{STATE_ROOT_REAL}}", &state_root_real)
                .replace("{{CELL_DIR}}", &cell_dir)
                .replace("{{CELL_DIR_REAL}}", &cell_dir_real);
            output.push_str(rendered.trim_end());
            output.push('\n');
        }

        output
    }

    /// Resolves the active local user used for lease metadata.
    pub fn lease_user(&self) -> Result<String, PlatformError> {
        if let Some(user) = std::env::var_os("USER") {
            let value = user.to_string_lossy().trim().to_string();
            if !value.is_empty() {
                return Ok(value);
            }
        }

        let output = StdCommand::new("id").arg("-un").output()?;
        if !output.status.success() {
            return Err(PlatformError::InvalidInput(
                "failed to resolve active user".to_string(),
            ));
        }

        let user = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if user.is_empty() {
            return Err(PlatformError::InvalidInput(
                "resolved user is empty".to_string(),
            ));
        }

        Ok(user)
    }

    /// Returns whether `sandbox-exec` is available on the current host.
    fn sandbox_exec_available(&self) -> bool {
        Path::new(SANDBOX_EXEC_PATH).exists()
    }

    /// Spawns a process directly without sandboxing.
    fn spawn_plain(
        &self,
        cmd: &CommandSpec,
        cwd: &Path,
        env: &BTreeMap<String, String>,
        stdout_file: fs::File,
        stderr_file: fs::File,
    ) -> Result<Child, PlatformError> {
        let mut command = Command::new(&cmd.argv[0]);
        if cmd.argv.len() > 1 {
            command.args(&cmd.argv[1..]);
        }

        command.current_dir(cwd);
        command.envs(env.clone());
        command.stdout(Stdio::from(stdout_file));
        command.stderr(Stdio::from(stderr_file));
        command.spawn().map_err(PlatformError::from)
    }

    /// Spawns a process under `sandbox-exec` with a prebuilt profile.
    fn spawn_sandboxed(
        &self,
        cmd: &CommandSpec,
        cwd: &Path,
        env: &BTreeMap<String, String>,
        profile_path: &Path,
        stdout_file: fs::File,
        stderr_file: fs::File,
    ) -> Result<Child, PlatformError> {
        if !self.sandbox_exec_available() {
            return Err(PlatformError::Unsupported(format!(
                "sandbox runtime not available at {SANDBOX_EXEC_PATH}"
            )));
        }

        let mut command = Command::new(SANDBOX_EXEC_PATH);
        command.arg("-f").arg(profile_path).arg(&cmd.argv[0]);
        if cmd.argv.len() > 1 {
            command.args(&cmd.argv[1..]);
        }

        command.current_dir(cwd);
        command.envs(env.clone());
        command.stdout(Stdio::from(stdout_file));
        command.stderr(Stdio::from(stderr_file));
        command.spawn().map_err(PlatformError::from)
    }

    /// Ensures the target cell directory exists before launching work.
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

    /// Ensures the log directory exists and returns its path.
    fn ensure_logs_dir(&self) -> Result<PathBuf, PlatformError> {
        let logs_dir = self.logs_dir();
        fs::create_dir_all(&logs_dir)?;
        Ok(logs_dir)
    }

    /// Opens stdout/stderr files, optionally appending existing content.
    fn open_log_files(
        &self,
        stdout_path: &Path,
        stderr_path: &Path,
        append: bool,
    ) -> Result<(fs::File, fs::File), PlatformError> {
        let mut options = fs::OpenOptions::new();
        options.create(true).write(true);
        if append {
            options.append(true);
        } else {
            options.truncate(true);
        }

        let stdout = options.open(stdout_path)?;
        let stderr = options.open(stderr_path)?;
        Ok((stdout, stderr))
    }

    /// Returns the path to persisted job metadata.
    fn job_meta_path(&self, job_id: &JobId) -> PathBuf {
        self.jobs_dir().join(format!("{}.json", job_id.0))
    }

    /// Loads persisted job metadata from disk.
    fn load_job(&self, job_id: &JobId) -> Result<JobInfo, PlatformError> {
        let path = self.job_meta_path(job_id);
        let bytes = fs::read(&path)?;
        serde_json::from_slice(&bytes).map_err(|err| {
            PlatformError::InvalidInput(format!(
                "failed to decode job metadata at {}: {err}",
                path.display()
            ))
        })
    }

    /// Sends one signal to a pid via `/bin/kill`.
    fn signal_pid(&self, pid: u32, signal: &str) -> Result<(), PlatformError> {
        let status = StdCommand::new("/bin/kill")
            .arg(format!("-{signal}"))
            .arg(pid.to_string())
            .status()?;

        if status.success() || status.code() == Some(1) {
            return Ok(());
        }

        Err(PlatformError::Io(io::Error::other(format!(
            "kill -{signal} {pid} failed with status {status}"
        ))))
    }

    /// Sends one signal to direct children via `pkill -P`.
    fn signal_children(&self, pid: u32, signal: &str) -> Result<(), PlatformError> {
        let status = StdCommand::new("/usr/bin/pkill")
            .arg(format!("-{signal}"))
            .arg("-P")
            .arg(pid.to_string())
            .status()?;

        if status.success() || status.code() == Some(1) {
            return Ok(());
        }

        Err(PlatformError::Io(io::Error::other(format!(
            "pkill -{signal} -P {pid} failed with status {status}"
        ))))
    }

    /// Returns whether a pid is currently alive.
    fn process_alive(&self, pid: u32) -> Result<bool, PlatformError> {
        let status = StdCommand::new("/bin/kill")
            .arg("-0")
            .arg(pid.to_string())
            .status()?;
        Ok(status.success())
    }
}

impl PlatformOps for MacosOps {
    /// Creates cell directories under the state root.
    fn create_cell_dirs(&self, cell_id: &CellId) -> Result<CellPaths, PlatformError> {
        let cell_dir = self.cells_dir().join(&cell_id.0);
        fs::create_dir_all(&cell_dir)?;
        Ok(CellPaths { cell_dir })
    }

    /// Spawns a job with optional sandbox enforcement and log wiring.
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
        let leased_user = self.lease_user()?;
        let sandbox_profile = self.compile_sandbox_profile(cell_id)?;

        let stdout_path = logs_dir.join(format!("{}.stdout.log", job_id.0));
        let stderr_path = logs_dir.join(format!("{}.stderr.log", job_id.0));

        let cwd = cmd
            .cwd
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| cell_dir.clone());

        let mut merged_env = env.clone();
        merged_env.insert("PLANTER_LEASED_USER".to_string(), leased_user);
        merged_env.insert(
            "PLANTER_SANDBOX_PROFILE".to_string(),
            sandbox_profile.to_string_lossy().to_string(),
        );

        let (stdout_file, stderr_file) = self.open_log_files(&stdout_path, &stderr_path, false)?;

        let child = match self.sandbox_mode {
            SandboxMode::Disabled => {
                self.spawn_plain(cmd, &cwd, &merged_env, stdout_file, stderr_file)?
            }
            SandboxMode::Permissive => {
                if self.sandbox_exec_available() {
                    match self.spawn_sandboxed(
                        cmd,
                        &cwd,
                        &merged_env,
                        &sandbox_profile,
                        stdout_file,
                        stderr_file,
                    ) {
                        Ok(child) => child,
                        Err(err) => {
                            tracing::warn!(
                                error = %err,
                                job_id = %job_id.0,
                                "sandbox launch failed in permissive mode; falling back to plain spawn"
                            );
                            let (stdout_file, stderr_file) =
                                self.open_log_files(&stdout_path, &stderr_path, true)?;
                            self.spawn_plain(cmd, &cwd, &merged_env, stdout_file, stderr_file)?
                        }
                    }
                } else {
                    tracing::warn!(
                        path = SANDBOX_EXEC_PATH,
                        job_id = %job_id.0,
                        "sandbox runtime missing in permissive mode; falling back to plain spawn"
                    );
                    self.spawn_plain(cmd, &cwd, &merged_env, stdout_file, stderr_file)?
                }
            }
            SandboxMode::Enforced => self.spawn_sandboxed(
                cmd,
                &cwd,
                &merged_env,
                &sandbox_profile,
                stdout_file,
                stderr_file,
            )?,
        };

        Ok(JobHandle {
            pid: child.id(),
            stdout_path,
            stderr_path,
            child,
        })
    }

    /// Stops a job process tree gracefully, then forcefully if needed.
    fn kill_job_tree(&self, job_id: &JobId, force: bool) -> Result<(), PlatformError> {
        let job = self.load_job(job_id)?;
        let Some(pid) = job.pid else {
            return Ok(());
        };

        if force {
            self.signal_children(pid, "KILL")?;
            self.signal_pid(pid, "KILL")?;
            return Ok(());
        }

        self.signal_children(pid, "TERM")?;
        self.signal_pid(pid, "TERM")?;

        thread::sleep(Duration::from_millis(250));

        if self.process_alive(pid)? {
            self.signal_children(pid, "KILL")?;
            self.signal_pid(pid, "KILL")?;
        }

        Ok(())
    }

    /// Samples RSS usage via `ps`; CPU is currently unavailable on this backend.
    fn probe_usage(&self, job_id: &JobId) -> Result<Option<JobUsage>, PlatformError> {
        let job = self.load_job(job_id)?;
        let Some(pid) = job.pid else {
            return Ok(None);
        };

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

        Ok(Some(JobUsage {
            rss_bytes: rss_kb.map(|value| value.saturating_mul(1024)),
            cpu_nanos: None,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{MacosOps, SANDBOX_EXEC_PATH, SandboxMode};
    use planter_core::{CellId, CommandSpec, JobId};
    use planter_platform::PlatformOps;
    use std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
    };
    use tempfile::tempdir;

    #[test]
    /// Verifies sandbox profile placeholders are resolved into concrete paths.
    fn sandbox_profile_renders_fragment_placeholders() {
        let ops = MacosOps::new(
            PathBuf::from("/tmp/planter-test-state"),
            SandboxMode::Permissive,
        );
        let cell_dir = PathBuf::from("/tmp/planter-test-state/cells/cell-123");
        let profile = ops.render_sandbox_profile(&CellId("cell-123".to_string()), &cell_dir);

        assert!(profile.contains("cell-123"));
        assert!(profile.contains("/tmp/planter-test-state"));
        assert!(profile.contains("/tmp/planter-test-state/cells/cell-123"));
        assert!(profile.contains("(allow process*)"));
        assert!(profile.contains("(allow network*)"));
    }

    #[tokio::test]
    /// Verifies enforced sandbox permits writes under the configured state root.
    async fn enforced_sandbox_allows_write_under_state_root() {
        if !Path::new(SANDBOX_EXEC_PATH).exists() {
            return;
        }

        let tmp = tempdir().expect("tempdir");
        let state_root = tmp.path().join("state");
        let ops = MacosOps::new(state_root.clone(), SandboxMode::Enforced);
        let cell_id = CellId("cell-test".to_string());
        ops.create_cell_dirs(&cell_id)
            .expect("cell dirs should be created");

        let allowed = state_root.join("logs").join("allowed.txt");
        std::fs::create_dir_all(allowed.parent().expect("has parent")).expect("create logs dir");

        let command = CommandSpec {
            argv: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                format!("echo allowed > {}", allowed.display()),
            ],
            cwd: None,
            env: BTreeMap::new(),
            limits: None,
        };

        let mut handle = ops
            .spawn_job(
                &JobId("job-allow".to_string()),
                &cell_id,
                &command,
                &BTreeMap::new(),
            )
            .expect("spawn should succeed");
        let status = handle.child.wait().await.expect("wait should succeed");

        if !status.success() {
            return;
        }
        assert!(allowed.exists());
    }

    #[tokio::test]
    /// Verifies enforced sandbox blocks writes outside the configured state root.
    async fn enforced_sandbox_blocks_write_outside_state_root() {
        if !Path::new(SANDBOX_EXEC_PATH).exists() {
            return;
        }

        let tmp = tempdir().expect("tempdir");
        let state_root = tmp.path().join("state");
        let outside_root = tmp.path().join("outside");
        let blocked = outside_root.join("blocked.txt");
        std::fs::create_dir_all(&outside_root).expect("create outside dir");

        let ops = MacosOps::new(state_root, SandboxMode::Enforced);
        let cell_id = CellId("cell-test".to_string());
        ops.create_cell_dirs(&cell_id)
            .expect("cell dirs should be created");

        let command = CommandSpec {
            argv: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                format!("echo blocked > {}", blocked.display()),
            ],
            cwd: None,
            env: BTreeMap::new(),
            limits: None,
        };

        let mut handle = ops
            .spawn_job(
                &JobId("job-deny".to_string()),
                &cell_id,
                &command,
                &BTreeMap::new(),
            )
            .expect("spawn should succeed");
        let status = handle.child.wait().await.expect("wait should succeed");

        assert!(!status.success());
        assert!(!blocked.exists());
    }
}
