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

const PROFILE_FRAGMENTS: &[(&str, &str)] = &[
    ("00-header", include_str!("../profiles/00-header.sb")),
    ("10-process", include_str!("../profiles/10-process.sb")),
    (
        "20-filesystem",
        include_str!("../profiles/20-filesystem.sb"),
    ),
    ("30-network", include_str!("../profiles/30-network.sb")),
];

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

    fn jobs_dir(&self) -> PathBuf {
        self.root.join("jobs")
    }

    fn sandbox_dir(&self) -> PathBuf {
        self.root.join("sandbox")
    }

    pub fn compile_sandbox_profile(&self, cell_id: &CellId) -> Result<PathBuf, PlatformError> {
        let sandbox_dir = self.sandbox_dir();
        fs::create_dir_all(&sandbox_dir)?;

        let profile_path = sandbox_dir.join(format!("{}.sb", cell_id.0));
        let profile = self.render_sandbox_profile(cell_id);
        fs::write(&profile_path, profile)?;

        Ok(profile_path)
    }

    fn render_sandbox_profile(&self, cell_id: &CellId) -> String {
        let mut output = String::new();
        let state_root = self.root.to_string_lossy().to_string();

        for (name, fragment) in PROFILE_FRAGMENTS {
            if !output.is_empty() {
                output.push('\n');
            }

            output.push_str("; ---- ");
            output.push_str(name);
            output.push_str(" ----\n");

            let rendered = fragment
                .replace("{{CELL_ID}}", &cell_id.0)
                .replace("{{STATE_ROOT}}", &state_root);
            output.push_str(rendered.trim_end());
            output.push('\n');
        }

        output
    }

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

    fn spawn_sandboxed(&self, command: &mut Command) -> Result<Child, PlatformError> {
        // v1: non-sandboxed spawn while sandbox profile generation is being validated.
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

    fn job_meta_path(&self, job_id: &JobId) -> PathBuf {
        self.jobs_dir().join(format!("{}.json", job_id.0))
    }

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

    fn process_alive(&self, pid: u32) -> Result<bool, PlatformError> {
        let status = StdCommand::new("/bin/kill")
            .arg("-0")
            .arg(pid.to_string())
            .status()?;
        Ok(status.success())
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
        let leased_user = self.lease_user()?;
        let sandbox_profile = self.compile_sandbox_profile(cell_id)?;

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
        command.env("PLANTER_LEASED_USER", leased_user);
        command.env(
            "PLANTER_SANDBOX_PROFILE",
            sandbox_profile.to_string_lossy().to_string(),
        );
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

    fn kill_job_tree(&self, job_id: &JobId) -> Result<(), PlatformError> {
        let job = self.load_job(job_id)?;
        let Some(pid) = job.pid else {
            return Ok(());
        };

        self.signal_children(pid, "TERM")?;
        self.signal_pid(pid, "TERM")?;

        thread::sleep(Duration::from_millis(250));

        if self.process_alive(pid)? {
            self.signal_children(pid, "KILL")?;
            self.signal_pid(pid, "KILL")?;
        }

        Ok(())
    }

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
    use super::MacosOps;
    use planter_core::CellId;
    use std::path::PathBuf;

    #[test]
    fn sandbox_profile_renders_fragment_placeholders() {
        let ops = MacosOps::new(PathBuf::from("/tmp/planter-test-state"));
        let profile = ops.render_sandbox_profile(&CellId("cell-123".to_string()));

        assert!(profile.contains("cell-123"));
        assert!(profile.contains("/tmp/planter-test-state"));
        assert!(profile.contains("(allow process*)"));
        assert!(profile.contains("(allow network*)"));
    }
}
