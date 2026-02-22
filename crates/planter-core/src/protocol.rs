use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{CellId, ErrorCode, JobId, ReqId, SessionId};

/// Wire protocol version expected by current binaries.
pub const PROTOCOL_VERSION: u32 = 2;

/// Request envelope carrying metadata plus a typed request body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestEnvelope<T> {
    /// Client-generated request identifier.
    pub req_id: ReqId,
    /// Typed request payload.
    pub body: T,
}

/// Response envelope carrying metadata plus a typed response body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseEnvelope<T> {
    /// Request identifier echoed from the request envelope.
    pub req_id: ReqId,
    /// Typed response payload.
    pub body: T,
}

/// Defines a new cell's metadata and base environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellSpec {
    /// Friendly cell name.
    pub name: String,
    /// Environment variables applied to all cell jobs.
    pub env: BTreeMap<String, String>,
}

/// Optional limits that apply to a launched job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Maximum runtime in milliseconds before timeout.
    pub timeout_ms: Option<u64>,
    /// Maximum resident set size in bytes.
    pub max_rss_bytes: Option<u64>,
    /// Maximum accumulated log bytes across streams.
    pub max_log_bytes: Option<u64>,
}

/// Command launch specification for job execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSpec {
    /// Executable and argument vector.
    pub argv: Vec<String>,
    /// Optional working directory.
    pub cwd: Option<String>,
    /// Per-command environment overrides.
    pub env: BTreeMap<String, String>,
    /// Optional resource limits.
    #[serde(default)]
    pub limits: Option<ResourceLimits>,
}

/// Materialized metadata for a created cell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellInfo {
    /// Stable cell identifier.
    pub id: CellId,
    /// Source specification used to create the cell.
    pub spec: CellSpec,
    /// Creation timestamp in UNIX milliseconds.
    pub created_at_ms: u64,
    /// Absolute path to the cell directory.
    pub dir: String,
}

/// Why a job transitioned out of running state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminationReason {
    /// Process exited on its own.
    Exited,
    /// User requested a graceful termination.
    TerminatedByUser,
    /// User requested force kill.
    ForcedKill,
    /// Runtime timeout limit was reached.
    Timeout,
    /// Memory limit was exceeded.
    MemoryLimit,
    /// Log quota was exceeded.
    LogQuota,
    /// Cause was not determined.
    Unknown,
}

/// Current process completion state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExitStatus {
    /// Process is still running.
    Running,
    /// Process has exited with an optional code.
    Exited { code: Option<i32> },
}

/// Log stream selector for read operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogStream {
    /// Standard output stream.
    Stdout,
    /// Standard error stream.
    Stderr,
}

/// PTY operation acknowledged by the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PtyAction {
    /// Session was created.
    Opened,
    /// Input bytes were accepted.
    Input,
    /// Terminal dimensions were updated.
    Resize,
    /// Session was closed.
    Closed,
}

/// Materialized metadata for a launched job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobInfo {
    /// Stable job identifier.
    pub id: JobId,
    /// Parent cell identifier.
    pub cell_id: CellId,
    /// Command specification used for launch.
    pub command: CommandSpec,
    /// Start timestamp in UNIX milliseconds.
    pub started_at_ms: u64,
    /// Optional finish timestamp in UNIX milliseconds.
    pub finished_at_ms: Option<u64>,
    /// Child process id when known.
    pub pid: Option<u32>,
    /// Current job exit status.
    pub status: ExitStatus,
    /// Optional reason for termination.
    #[serde(default)]
    pub termination_reason: Option<TerminationReason>,
}

/// RPC request variants supported by the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Returns daemon and protocol version metadata.
    Version {},
    /// Returns liveness status.
    Health {},
    /// Creates a new cell.
    CellCreate {
        /// Cell creation specification.
        spec: CellSpec,
    },
    /// Starts a new job within a cell.
    JobRun {
        /// Target cell identifier.
        cell_id: CellId,
        /// Command to execute.
        cmd: CommandSpec,
    },
    /// Fetches current job status.
    JobStatus {
        /// Target job identifier.
        job_id: JobId,
    },
    /// Requests job termination.
    JobKill {
        /// Target job identifier.
        job_id: JobId,
        /// When true, perform forceful termination.
        force: bool,
    },
    /// Removes a cell and optionally its active jobs.
    CellRemove {
        /// Target cell identifier.
        cell_id: CellId,
        /// When true, remove even if jobs are active.
        force: bool,
    },
    /// Reads job logs from a stream with offset-based pagination.
    LogsRead {
        /// Target job identifier.
        job_id: JobId,
        /// Selected stream.
        stream: LogStream,
        /// Byte offset to start reading from.
        offset: u64,
        /// Maximum bytes to return.
        max_bytes: u32,
        /// Whether to wait for additional bytes when at EOF.
        follow: bool,
        /// Follow wait timeout in milliseconds.
        wait_ms: u64,
    },
    /// Opens an interactive PTY session.
    PtyOpen {
        /// Shell binary path.
        shell: String,
        /// Shell argument vector.
        args: Vec<String>,
        /// Optional working directory.
        cwd: Option<String>,
        /// Environment overrides.
        env: BTreeMap<String, String>,
        /// Initial terminal columns.
        cols: u16,
        /// Initial terminal rows.
        rows: u16,
    },
    /// Sends input bytes to a PTY session.
    PtyInput {
        /// Target PTY session identifier.
        session_id: SessionId,
        /// Raw input bytes.
        data: Vec<u8>,
    },
    /// Reads PTY output from an offset.
    PtyRead {
        /// Target PTY session identifier.
        session_id: SessionId,
        /// Byte offset to start reading from.
        offset: u64,
        /// Maximum bytes to return.
        max_bytes: u32,
        /// Whether to wait for additional bytes when at EOF.
        follow: bool,
        /// Follow wait timeout in milliseconds.
        wait_ms: u64,
    },
    /// Resizes an existing PTY session.
    PtyResize {
        /// Target PTY session identifier.
        session_id: SessionId,
        /// Terminal columns.
        cols: u16,
        /// Terminal rows.
        rows: u16,
    },
    /// Closes a PTY session.
    PtyClose {
        /// Target PTY session identifier.
        session_id: SessionId,
        /// When true, force-close the session.
        force: bool,
    },
}

/// RPC response variants returned by the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// Protocol version details.
    Version {
        /// Daemon version string.
        daemon: String,
        /// Protocol version number.
        protocol: u32,
    },
    /// Service health result.
    Health {
        /// Health status string.
        status: String,
    },
    /// Cell creation acknowledgment.
    CellCreated {
        /// Created cell metadata.
        cell: CellInfo,
    },
    /// Job start acknowledgment.
    JobStarted {
        /// Started job metadata.
        job: JobInfo,
    },
    /// Job status payload.
    JobStatus {
        /// Current job metadata.
        job: JobInfo,
    },
    /// Job termination acknowledgment.
    JobKilled {
        /// Terminated job identifier.
        job_id: JobId,
        /// Signal description used for termination.
        signal: String,
        /// Latest job status after signal delivery.
        status: ExitStatus,
    },
    /// Cell removal acknowledgment.
    CellRemoved {
        /// Removed cell identifier.
        cell_id: CellId,
    },
    /// Chunk of job log output.
    LogsChunk {
        /// Job identifier.
        job_id: JobId,
        /// Stream from which bytes were read.
        stream: LogStream,
        /// Offset immediately after this chunk.
        offset: u64,
        /// Raw log bytes.
        data: Vec<u8>,
        /// True when no more bytes are currently available.
        eof: bool,
        /// True when the source stream is complete and closed.
        complete: bool,
    },
    /// PTY open acknowledgment.
    PtyOpened {
        /// Opened PTY session identifier.
        session_id: SessionId,
        /// Shell process id when known.
        pid: Option<u32>,
    },
    /// Chunk of PTY output data.
    PtyChunk {
        /// PTY session identifier.
        session_id: SessionId,
        /// Offset immediately after this chunk.
        offset: u64,
        /// Raw PTY bytes.
        data: Vec<u8>,
        /// True when no more bytes are currently available.
        eof: bool,
        /// True when the PTY process has exited.
        complete: bool,
        /// Exit code when complete.
        exit_code: Option<i32>,
    },
    /// PTY control acknowledgment.
    PtyAck {
        /// PTY session identifier.
        session_id: SessionId,
        /// Operation that was acknowledged.
        action: PtyAction,
    },
    /// Point-in-time resource usage sample.
    UsageSample {
        /// Job identifier.
        job_id: JobId,
        /// Resident set size in bytes.
        rss_bytes: Option<u64>,
        /// CPU usage in nanoseconds.
        cpu_nanos: Option<u64>,
        /// Sample timestamp in UNIX milliseconds.
        timestamp_ms: u64,
    },
    /// Structured error response.
    Error {
        /// High-level error category.
        code: ErrorCode,
        /// Human-readable summary.
        message: String,
        /// Optional extended context.
        detail: Option<String>,
    },
}
