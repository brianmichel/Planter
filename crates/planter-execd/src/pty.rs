//! PTY session management with optional nested sandbox execution.

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command as StdCommand,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use planter_core::{ErrorCode, PlanterError, SessionId};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::time::sleep;

/// Policy controlling whether PTY shells are nested inside `sandbox-exec`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum PtySandboxMode {
    /// Never attempt nested sandboxing.
    Disabled,
    /// Attempt nested sandboxing where possible.
    Permissive,
    /// Require nested sandboxing unless blocked by parent confinement.
    Enforced,
}

/// Path to the system sandbox launcher.
const SANDBOX_EXEC_PATH: &str = "/usr/bin/sandbox-exec";
/// Minimal profile used to probe nested sandbox support.
const NESTED_SANDBOX_PROBE_PROFILE: &str = "(version 1) (allow default)";
/// Shared sandbox profile fragments used to build PTY profiles.
const PROFILE_FRAGMENTS: &[(&str, &str)] = &[
    (
        "00-header",
        include_str!("../../planter-platform-macos/profiles/00-header.sb"),
    ),
    (
        "10-process",
        include_str!("../../planter-platform-macos/profiles/10-process.sb"),
    ),
    (
        "20-filesystem",
        include_str!("../../planter-platform-macos/profiles/20-filesystem.sb"),
    ),
    (
        "30-network",
        include_str!("../../planter-platform-macos/profiles/30-network.sb"),
    ),
];

/// Result of probing whether nested sandboxing can be applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NestedSandboxCapability {
    /// Nested sandboxing works.
    Available,
    /// Parent sandbox blocks nested `sandbox-exec`.
    BlockedByParentSandbox,
}

/// Manages lifecycle and I/O for interactive PTY sessions.
pub struct PtyManager {
    /// Root path for session filesystem state.
    state_root: PathBuf,
    /// Runtime sandbox policy.
    sandbox_mode: PtySandboxMode,
    /// Active sessions by id.
    sessions: Mutex<HashMap<SessionId, Arc<PtySession>>>,
    /// Monotonic session id generator.
    next_id: AtomicU64,
}

/// Result payload for PTY open operations.
pub struct PtyOpenResult {
    /// Newly created session id.
    pub session_id: SessionId,
    /// Child process id when available.
    pub pid: Option<u32>,
}

/// Result payload for PTY read operations.
pub struct PtyReadResult {
    /// Offset used for the read call.
    pub offset: u64,
    /// Raw output bytes.
    pub data: Vec<u8>,
    /// True when no more bytes are currently buffered.
    pub eof: bool,
    /// True when session has fully completed.
    pub complete: bool,
    /// Exit code when complete.
    pub exit_code: Option<i32>,
}

/// In-memory state for a single PTY session.
struct PtySession {
    /// Writable PTY input stream.
    writer: Mutex<Box<dyn Write + Send>>,
    /// PTY master handle for control operations.
    master: Mutex<Box<dyn MasterPty + Send>>,
    /// Child process handle.
    child: Mutex<Box<dyn Child + Send>>,
    /// Buffered PTY output bytes.
    buffer: Mutex<Vec<u8>>,
    /// Completion marker for the reader thread.
    complete: AtomicBool,
    /// Captured process exit code.
    exit_code: Mutex<Option<i32>>,
}

impl PtyManager {
    /// Creates an empty PTY manager for the provided state root.
    pub fn new(state_root: PathBuf, sandbox_mode: PtySandboxMode) -> Self {
        Self {
            state_root,
            sandbox_mode,
            sessions: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }

    /// Opens a new PTY session and spawns the requested shell command.
    pub fn open(
        &self,
        shell: String,
        args: Vec<String>,
        cwd: Option<String>,
        env: BTreeMap<String, String>,
        cols: u16,
        rows: u16,
    ) -> Result<PtyOpenResult, PlanterError> {
        if shell.trim().is_empty() {
            return Err(PlanterError {
                code: ErrorCode::InvalidRequest,
                message: "shell cannot be empty".to_string(),
                detail: None,
            });
        }
        validate_shell_path(&shell)?;

        let session_id = SessionId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let layout = self.prepare_layout(session_id)?;
        let shell_args = normalize_shell_args(&shell, &layout, args);
        let cwd = cwd.unwrap_or_else(|| layout.build_cell.display().to_string());
        let env = build_isolated_env(&shell, &layout, cwd.clone(), env);
        let (program, program_args) =
            self.resolve_spawn_command(session_id, &layout, &shell, shell_args)?;
        let launched_with_sandbox = program == SANDBOX_EXEC_PATH;

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| pty_to_error("open pty", err.to_string()))?;

        let mut command = CommandBuilder::new(program);
        for arg in program_args {
            command.arg(arg);
        }
        command.cwd(cwd);
        command.env_clear();
        for (key, value) in env {
            command.env(key, value);
        }

        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|err| pty_to_error("spawn pty command", err.to_string()))?;
        if launched_with_sandbox && self.sandbox_mode == PtySandboxMode::Enforced {
            std::thread::sleep(Duration::from_millis(50));
            let exited_early = child
                .try_wait()
                .map_err(|err| pty_to_error("probe sandboxed pty process", err.to_string()))?;
            if let Some(status) = exited_early {
                let mut detail = format!("exit_code={}", status.exit_code());
                if let Some(startup_output) = read_startup_output(&*pair.master, 512) {
                    detail.push_str(", startup_output=");
                    detail.push_str(&startup_output);
                }
                return Err(PlanterError {
                    code: ErrorCode::Internal,
                    message: "sandboxed pty shell exited during startup".to_string(),
                    detail: Some(detail),
                });
            }
        }
        let pid = child.process_id();

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|err| pty_to_error("clone pty reader", err.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|err| pty_to_error("take pty writer", err.to_string()))?;

        let session = Arc::new(PtySession {
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            child: Mutex::new(child),
            buffer: Mutex::new(Vec::new()),
            complete: AtomicBool::new(false),
            exit_code: Mutex::new(None),
        });

        spawn_reader_thread(Arc::clone(&session), reader);

        self.sessions
            .lock()
            .map_err(|_| lock_error("sessions lock poisoned"))?
            .insert(session_id, session);

        Ok(PtyOpenResult { session_id, pid })
    }

    /// Sends raw input bytes to an active session.
    pub fn input(&self, session_id: SessionId, data: Vec<u8>) -> Result<(), PlanterError> {
        if data.is_empty() {
            return Ok(());
        }

        let session = self.get_session(session_id)?;
        let mut writer = session
            .writer
            .lock()
            .map_err(|_| lock_error("pty writer lock poisoned"))?;
        writer
            .write_all(&data)
            .map_err(|err| pty_to_error("write pty input", err.to_string()))?;
        writer
            .flush()
            .map_err(|err| pty_to_error("flush pty input", err.to_string()))
    }

    /// Reads PTY output using offset pagination with optional follow behavior.
    pub async fn read(
        &self,
        session_id: SessionId,
        offset: u64,
        max_bytes: u32,
        follow: bool,
        wait_ms: u64,
    ) -> Result<PtyReadResult, PlanterError> {
        let start = Instant::now();
        let max_bytes = usize::try_from(max_bytes.max(1)).unwrap_or(64 * 1024);

        loop {
            let session = self.get_session(session_id)?;
            let chunk = session.read_chunk(offset, max_bytes)?;

            if !chunk.data.is_empty() || chunk.complete || !follow {
                return Ok(chunk);
            }

            if start.elapsed() >= Duration::from_millis(wait_ms.max(1)) {
                return Ok(chunk);
            }

            sleep(Duration::from_millis(50)).await;
        }
    }

    /// Resizes the PTY terminal dimensions for an active session.
    pub fn resize(&self, session_id: SessionId, cols: u16, rows: u16) -> Result<(), PlanterError> {
        let session = self.get_session(session_id)?;
        let master = session
            .master
            .lock()
            .map_err(|_| lock_error("pty master lock poisoned"))?;
        master
            .resize(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| pty_to_error("resize pty", err.to_string()))
    }

    /// Closes a PTY session and terminates its child process.
    pub fn close(&self, session_id: SessionId, force: bool) -> Result<(), PlanterError> {
        let session = self
            .sessions
            .lock()
            .map_err(|_| lock_error("sessions lock poisoned"))?
            .remove(&session_id)
            .ok_or_else(|| not_found_error(format!("session {} does not exist", session_id.0)))?;

        {
            let mut child = session
                .child
                .lock()
                .map_err(|_| lock_error("pty child lock poisoned"))?;
            child
                .kill()
                .map_err(|err| pty_to_error("kill pty process", err.to_string()))?;
            if force {
                let _ = child.kill();
            }
        }

        session.complete.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Retrieves a cloned session handle by id.
    fn get_session(&self, session_id: SessionId) -> Result<Arc<PtySession>, PlanterError> {
        self.sessions
            .lock()
            .map_err(|_| lock_error("sessions lock poisoned"))?
            .get(&session_id)
            .cloned()
            .ok_or_else(|| not_found_error(format!("session {} does not exist", session_id.0)))
    }

    /// Creates per-session filesystem layout and startup rc files.
    fn prepare_layout(&self, session_id: SessionId) -> Result<SessionLayout, PlanterError> {
        let session_root = self
            .state_root
            .join("sessions")
            .join(format!("pty-{}", session_id.0));
        let build_cell = session_root.join("build-cell");
        let session_home = session_root.join("home");
        let session_tmp = session_root.join("tmp");
        let bash_rc = session_home.join(".planter_bashrc");
        let zsh_rc = session_home.join(".zshrc");

        fs::create_dir_all(&build_cell)
            .map_err(|err| pty_to_error("create build-cell directory", err.to_string()))?;
        fs::create_dir_all(&session_home)
            .map_err(|err| pty_to_error("create session home directory", err.to_string()))?;
        fs::create_dir_all(&session_tmp)
            .map_err(|err| pty_to_error("create session tmp directory", err.to_string()))?;
        fs::write(&bash_rc, render_bash_rc(&build_cell).as_bytes())
            .map_err(|err| pty_to_error("write session bash rc", err.to_string()))?;
        fs::write(&zsh_rc, render_zsh_rc(&build_cell).as_bytes())
            .map_err(|err| pty_to_error("write session zsh rc", err.to_string()))?;

        Ok(SessionLayout {
            build_cell,
            session_root,
            session_home,
            session_tmp,
            bash_rc,
        })
    }

    /// Resolves the executable and argument vector used to spawn the PTY shell.
    fn resolve_spawn_command(
        &self,
        session_id: SessionId,
        layout: &SessionLayout,
        shell: &str,
        shell_args: Vec<String>,
    ) -> Result<(String, Vec<String>), PlanterError> {
        match self.sandbox_mode {
            PtySandboxMode::Disabled => Ok((shell.to_string(), shell_args)),
            PtySandboxMode::Permissive => {
                tracing::debug!(
                    session_id = session_id.0,
                    "pty sandbox is skipped in permissive mode; using plain shell"
                );
                Ok((shell.to_string(), shell_args))
            }
            PtySandboxMode::Enforced => match probe_nested_sandbox_capability()? {
                NestedSandboxCapability::Available => {
                    self.sandbox_launch_prefix(session_id, layout, shell, &shell_args)
                }
                NestedSandboxCapability::BlockedByParentSandbox => {
                    tracing::warn!(
                        session_id = session_id.0,
                        "nested sandbox-exec unavailable under current confinement; using direct shell launch"
                    );
                    Ok((shell.to_string(), shell_args))
                }
            },
        }
    }

    /// Builds the `sandbox-exec` command prefix for enforced sandbox launches.
    fn sandbox_launch_prefix(
        &self,
        session_id: SessionId,
        layout: &SessionLayout,
        shell: &str,
        shell_args: &[String],
    ) -> Result<(String, Vec<String>), PlanterError> {
        if !Path::new(SANDBOX_EXEC_PATH).exists() {
            return Err(PlanterError {
                code: ErrorCode::Internal,
                message: "sandbox runtime unavailable".to_string(),
                detail: Some(format!("missing {}", SANDBOX_EXEC_PATH)),
            });
        }

        let profile_path = self.compile_sandbox_profile(session_id, layout)?;
        let mut args = vec![
            "-f".to_string(),
            profile_path.to_string_lossy().to_string(),
            shell.to_string(),
        ];
        args.extend(shell_args.iter().cloned());
        Ok((SANDBOX_EXEC_PATH.to_string(), args))
    }

    /// Writes the generated sandbox profile for a PTY session.
    fn compile_sandbox_profile(
        &self,
        session_id: SessionId,
        layout: &SessionLayout,
    ) -> Result<PathBuf, PlanterError> {
        let sandbox_dir = self.state_root.join("sandbox");
        fs::create_dir_all(&sandbox_dir)
            .map_err(|err| pty_to_error("create pty sandbox directory", err.to_string()))?;

        let profile_path = sandbox_dir.join(format!("pty-{}.sb", session_id.0));
        let profile = render_sandbox_profile(
            &self.state_root,
            &layout.session_home,
            &layout.build_cell,
            session_id,
        );
        fs::write(&profile_path, profile)
            .map_err(|err| pty_to_error("write pty sandbox profile", err.to_string()))?;
        Ok(profile_path)
    }
}

impl PtySession {
    /// Reads a buffered output chunk and session completion metadata.
    fn read_chunk(&self, offset: u64, max_bytes: usize) -> Result<PtyReadResult, PlanterError> {
        let buffer = self
            .buffer
            .lock()
            .map_err(|_| lock_error("pty buffer lock poisoned"))?;

        let len = buffer.len();
        let start = usize::try_from(offset).unwrap_or(len).min(len);
        let end = start.saturating_add(max_bytes).min(len);
        let data = buffer[start..end].to_vec();
        let eof = end >= len;
        let complete = eof && self.complete.load(Ordering::Relaxed);
        let exit_code = *self
            .exit_code
            .lock()
            .map_err(|_| lock_error("pty exit code lock poisoned"))?;

        Ok(PtyReadResult {
            offset,
            data,
            eof,
            complete,
            exit_code,
        })
    }
}

/// Spawns a background reader that copies PTY output into the session buffer.
fn spawn_reader_thread(session: Arc<PtySession>, mut reader: Box<dyn Read + Send>) {
    std::thread::spawn(move || {
        let mut buf = [0_u8; 4096];

        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if let Ok(mut bytes) = session.buffer.lock() {
                        bytes.extend_from_slice(&buf[..n]);
                    } else {
                        break;
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }

        session.complete.store(true, Ordering::Relaxed);
    });
}

/// Paths and files prepared for an individual PTY session.
struct SessionLayout {
    /// Session-local writable build cell.
    build_cell: PathBuf,
    /// Root path containing all per-session artifacts.
    session_root: PathBuf,
    /// Session HOME directory.
    session_home: PathBuf,
    /// Session temp directory.
    session_tmp: PathBuf,
    /// Generated bash rc file path.
    bash_rc: PathBuf,
}

/// Normalizes user-provided shell args and supplies safe defaults when absent.
fn normalize_shell_args(shell: &str, layout: &SessionLayout, args: Vec<String>) -> Vec<String> {
    if args.is_empty() || (args.len() == 1 && args[0] == "-i") {
        return default_shell_args(shell, layout);
    }

    args
}

/// Returns default interactive args tailored to known shells.
fn default_shell_args(shell: &str, layout: &SessionLayout) -> Vec<String> {
    match shell_name(shell).as_deref() {
        Some("zsh") => vec!["-d".to_string(), "-i".to_string()],
        Some("bash") => vec![
            "--noprofile".to_string(),
            "--rcfile".to_string(),
            layout.bash_rc.to_string_lossy().to_string(),
            "-i".to_string(),
        ],
        _ => vec!["-i".to_string()],
    }
}

/// Returns the lowercase shell basename from a shell path.
fn shell_name(shell: &str) -> Option<String> {
    Path::new(shell)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_ascii_lowercase())
}

/// Builds a constrained environment map for PTY shells.
fn build_isolated_env(
    shell: &str,
    layout: &SessionLayout,
    cwd: String,
    overrides: BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();

    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    if let Ok(term) = std::env::var("TERM") {
        env.insert("TERM".to_string(), term);
    } else {
        env.insert("TERM".to_string(), "xterm-256color".to_string());
    }
    if let Ok(lang) = std::env::var("LANG") {
        env.insert("LANG".to_string(), lang);
    }

    for (key, value) in overrides {
        if matches!(
            key.as_str(),
            "HOME"
                | "USER"
                | "LOGNAME"
                | "ZDOTDIR"
                | "HISTFILE"
                | "PWD"
                | "TMPDIR"
                | "TEMP"
                | "TMP"
        ) {
            continue;
        }
        env.insert(key, value);
    }

    env.insert(
        "HOME".to_string(),
        layout.session_home.display().to_string(),
    );
    env.insert("USER".to_string(), "anonymous".to_string());
    env.insert("LOGNAME".to_string(), "anonymous".to_string());
    env.insert(
        "ZDOTDIR".to_string(),
        layout.session_home.display().to_string(),
    );
    env.insert("HISTFILE".to_string(), "/dev/null".to_string());
    env.insert(
        "TMPDIR".to_string(),
        format!("{}/", layout.session_tmp.display()),
    );
    env.insert("TEMP".to_string(), layout.session_tmp.display().to_string());
    env.insert("TMP".to_string(), layout.session_tmp.display().to_string());
    env.insert("SHELL".to_string(), shell.to_string());
    env.insert("PWD".to_string(), cwd);
    env.insert(
        "PLANTER_BUILD_CELL".to_string(),
        layout.build_cell.display().to_string(),
    );
    env.insert(
        "PLANTER_SESSION_ROOT".to_string(),
        layout.session_root.display().to_string(),
    );

    env
}

/// Renders the bash startup script used inside PTY sessions.
fn render_bash_rc(build_cell: &Path) -> String {
    let build_cell = shell_single_quote(build_cell.to_string_lossy().as_ref());
    format!(
        r#"
export PLANTER_BUILD_CELL='{build_cell}'
builtin cd "$PLANTER_BUILD_CELL" 2>/dev/null || true
stty sane 2>/dev/null || true
stty erase '^?' 2>/dev/null || true
bind '"\C-h": backward-delete-char'
bind '"\C-?": backward-delete-char'
bind '"\e[3~": backward-delete-char'
cd() {{
  if [ "$#" -eq 0 ]; then
    builtin cd "$PLANTER_BUILD_CELL"
    return $?
  fi
  case "$1" in
    "$PLANTER_BUILD_CELL"|"$PLANTER_BUILD_CELL/"*)
      builtin cd "$1"
      ;;
    *)
      printf 'planter: blocked cd outside build cell: %s\n' "$1" >&2
      return 1
      ;;
  esac
}}
readonly -f cd
PROMPT_COMMAND='builtin cd "$PLANTER_BUILD_CELL" 2>/dev/null || true'
readonly PROMPT_COMMAND
PS1='planter:\w\$ '
"#
    )
}

/// Renders the zsh startup script used inside PTY sessions.
fn render_zsh_rc(build_cell: &Path) -> String {
    let build_cell = shell_single_quote(build_cell.to_string_lossy().as_ref());
    format!(
        r#"
export PLANTER_BUILD_CELL='{build_cell}'
builtin cd "$PLANTER_BUILD_CELL" 2>/dev/null || true
stty sane 2>/dev/null || true
stty erase '^?' 2>/dev/null || true
bindkey '^?' backward-delete-char
bindkey '^H' backward-delete-char
bindkey '\e[3~' backward-delete-char
bindkey -M emacs '^?' backward-delete-char
bindkey -M emacs '^H' backward-delete-char
bindkey -M emacs '\e[3~' backward-delete-char
bindkey -M viins '^?' backward-delete-char
bindkey -M viins '^H' backward-delete-char
bindkey -M viins '\e[3~' backward-delete-char
function cd() {{
  if [[ "$#" -eq 0 ]]; then
    builtin cd "$PLANTER_BUILD_CELL"
    return $?
  fi
  case "$1" in
    "$PLANTER_BUILD_CELL"|"$PLANTER_BUILD_CELL/"*)
      builtin cd "$1"
      ;;
    *)
      print -u2 -- "planter: blocked cd outside build cell: $1"
      return 1
      ;;
  esac
}}
function precmd() {{
  builtin cd "$PLANTER_BUILD_CELL" 2>/dev/null || true
}}
PROMPT='planter:%~ %# '
"#
    )
}

/// Escapes a value for safe inclusion in single-quoted shell strings.
fn shell_single_quote(value: &str) -> String {
    value.replace('\'', r#"'\''"#)
}

/// Validates that an absolute shell path exists and is executable.
fn validate_shell_path(shell: &str) -> Result<(), PlanterError> {
    let shell_path = Path::new(shell);
    if !shell_path.is_absolute() {
        return Ok(());
    }

    let metadata = fs::metadata(shell_path).map_err(|err| PlanterError {
        code: ErrorCode::InvalidRequest,
        message: "shell path is invalid".to_string(),
        detail: Some(format!("{shell}: {err}")),
    })?;

    if !metadata.is_file() {
        return Err(PlanterError {
            code: ErrorCode::InvalidRequest,
            message: "shell path is not a regular file".to_string(),
            detail: Some(shell.to_string()),
        });
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(PlanterError {
                code: ErrorCode::InvalidRequest,
                message: "shell path is not executable".to_string(),
                detail: Some(shell.to_string()),
            });
        }
    }

    Ok(())
}

/// Probes whether nested `sandbox-exec` can run under current confinement.
fn probe_nested_sandbox_capability() -> Result<NestedSandboxCapability, PlanterError> {
    let output = StdCommand::new(SANDBOX_EXEC_PATH)
        .arg("-p")
        .arg(NESTED_SANDBOX_PROBE_PROFILE)
        .arg("/usr/bin/true")
        .output();

    match output {
        Ok(output) if output.status.success() => Ok(NestedSandboxCapability::Available),
        Ok(output) => {
            if is_nested_sandbox_denied_by_parent(output.status.code(), &output.stderr) {
                return Ok(NestedSandboxCapability::BlockedByParentSandbox);
            }

            let mut detail = format!("exit_code={}", output.status.code().unwrap_or(-1));
            if let Some(stderr) = format_output_for_detail(&output.stderr, 256) {
                detail.push_str(", stderr=");
                detail.push_str(&stderr);
            }
            if let Some(stdout) = format_output_for_detail(&output.stdout, 256) {
                detail.push_str(", stdout=");
                detail.push_str(&stdout);
            }

            Err(PlanterError {
                code: ErrorCode::Internal,
                message: "probe nested sandbox support".to_string(),
                detail: Some(detail),
            })
        }
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
            Ok(NestedSandboxCapability::BlockedByParentSandbox)
        }
        Err(err) => Err(pty_to_error(
            "probe nested sandbox support",
            err.to_string(),
        )),
    }
}

/// Detects known stderr patterns for parent sandbox nested-apply denials.
fn is_nested_sandbox_denied_by_parent(exit_code: Option<i32>, stderr: &[u8]) -> bool {
    let stderr = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    (stderr.contains("sandbox_apply") && stderr.contains("operation not permitted"))
        || (exit_code == Some(71) && stderr.contains("operation not permitted"))
}

/// Normalizes command output for compact error details.
fn format_output_for_detail(output: &[u8], max_chars: usize) -> Option<String> {
    let compact = String::from_utf8_lossy(output)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if compact.is_empty() {
        return None;
    }

    Some(truncate_detail(&compact, max_chars))
}

/// Truncates verbose detail strings to a fixed character limit.
fn truncate_detail(value: &str, max_chars: usize) -> String {
    let mut truncated = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        truncated.push_str("...");
    }
    truncated
}

/// Reads startup output from a PTY master for early-exit diagnostics.
fn read_startup_output(master: &(dyn MasterPty + Send), max_chars: usize) -> Option<String> {
    let mut reader = master.try_clone_reader().ok()?;
    let mut bytes = Vec::new();
    if reader.read_to_end(&mut bytes).is_err() {
        return None;
    }

    format_output_for_detail(&bytes, max_chars)
}

/// Renders the sandbox profile text for one PTY session.
fn render_sandbox_profile(
    state_root: &Path,
    session_home: &Path,
    build_cell: &Path,
    session_id: SessionId,
) -> String {
    let mut output = String::new();
    let state_root_display = state_root.to_string_lossy().to_string();
    let state_root_real = fs::canonicalize(state_root)
        .unwrap_or_else(|_| state_root.to_path_buf())
        .to_string_lossy()
        .to_string();
    let session_home_display = session_home.to_string_lossy().to_string();
    let session_home_real = fs::canonicalize(session_home)
        .unwrap_or_else(|_| session_home.to_path_buf())
        .to_string_lossy()
        .to_string();
    let bash_rc = session_home.join(".planter_bashrc");
    let zsh_rc = session_home.join(".zshrc");
    let bash_rc_display = bash_rc.to_string_lossy().to_string();
    let zsh_rc_display = zsh_rc.to_string_lossy().to_string();
    let bash_rc_real = fs::canonicalize(&bash_rc)
        .unwrap_or(bash_rc)
        .to_string_lossy()
        .to_string();
    let zsh_rc_real = fs::canonicalize(&zsh_rc)
        .unwrap_or(zsh_rc)
        .to_string_lossy()
        .to_string();
    let cell_dir = build_cell.to_string_lossy().to_string();
    let cell_dir_real = fs::canonicalize(build_cell)
        .unwrap_or_else(|_| build_cell.to_path_buf())
        .to_string_lossy()
        .to_string();

    for (name, fragment) in PROFILE_FRAGMENTS {
        if !output.is_empty() {
            output.push('\n');
        }

        output.push_str("; ---- ");
        output.push_str(name);
        output.push_str(" ----\n");

        let rendered = fragment
            .replace("{{CELL_ID}}", &format!("pty-{}", session_id.0))
            .replace("{{STATE_ROOT}}", &state_root_display)
            .replace("{{STATE_ROOT_REAL}}", &state_root_real)
            .replace("{{CELL_DIR}}", &cell_dir)
            .replace("{{CELL_DIR_REAL}}", &cell_dir_real);

        output.push_str(rendered.trim_end());
        output.push('\n');
    }

    output.push_str("\n; ---- pty-rc-hardening ----\n");
    output.push_str(&format!(
        "(deny file-write* (literal \"{}\"))\n",
        bash_rc_display
    ));
    output.push_str(&format!(
        "(deny file-write* (literal \"{}\"))\n",
        zsh_rc_display
    ));
    output.push_str(&format!(
        "(deny file-write* (literal \"{}\"))\n",
        bash_rc_real
    ));
    output.push_str(&format!(
        "(deny file-write* (literal \"{}\"))\n",
        zsh_rc_real
    ));
    output.push_str(&format!(
        "(allow file-write* (subpath \"{}\"))\n",
        session_home_display
    ));
    output.push_str(&format!(
        "(allow file-write* (subpath \"{}\"))\n",
        session_home_real
    ));
    output.push_str("\n; ---- pty-metadata-compat ----\n");
    output.push_str("(allow file-read-metadata)\n");
    output.push_str("\n; ---- pty-devices ----\n");
    output.push_str("(allow file-read* (subpath \"/dev\"))\n");
    output.push_str("(allow file-write* (subpath \"/dev\"))\n");

    output
}

/// Builds a standardized internal PTY error payload.
fn pty_to_error(action: &str, detail: String) -> PlanterError {
    PlanterError {
        code: ErrorCode::Internal,
        message: action.to_string(),
        detail: Some(detail),
    }
}

/// Builds a standardized lock-poisoned error payload.
fn lock_error(message: &str) -> PlanterError {
    PlanterError {
        code: ErrorCode::Internal,
        message: message.to_string(),
        detail: None,
    }
}

/// Builds a standardized not-found error payload.
fn not_found_error(message: String) -> PlanterError {
    PlanterError {
        code: ErrorCode::NotFound,
        message,
        detail: None,
    }
}

#[cfg(test)]
mod tests {
    use super::is_nested_sandbox_denied_by_parent;

    #[test]
    /// Detects known stderr pattern for nested sandbox permission denial.
    fn detects_nested_sandbox_apply_denial_message() {
        let stderr = b"sandbox-exec: sandbox_apply: Operation not permitted";
        assert!(is_nested_sandbox_denied_by_parent(Some(71), stderr));
        assert!(is_nested_sandbox_denied_by_parent(Some(1), stderr));
    }

    #[test]
    /// Ensures unrelated sandbox failures are not classified as nested denials.
    fn ignores_unrelated_sandbox_failures() {
        let stderr = b"sandbox-exec: invalid profile";
        assert!(!is_nested_sandbox_denied_by_parent(Some(1), stderr));
    }
}
