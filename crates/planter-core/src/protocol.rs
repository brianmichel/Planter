use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{CellId, ErrorCode, JobId, ReqId, SessionId};

pub const PROTOCOL_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestEnvelope<T> {
    pub req_id: ReqId,
    pub body: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseEnvelope<T> {
    pub req_id: ReqId,
    pub body: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellSpec {
    pub name: String,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub timeout_ms: Option<u64>,
    pub max_rss_bytes: Option<u64>,
    pub max_log_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSpec {
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub limits: Option<ResourceLimits>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellInfo {
    pub id: CellId,
    pub spec: CellSpec,
    pub created_at_ms: u64,
    pub dir: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminationReason {
    Exited,
    TerminatedByUser,
    ForcedKill,
    Timeout,
    MemoryLimit,
    LogQuota,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExitStatus {
    Running,
    Exited { code: Option<i32> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PtyAction {
    Opened,
    Input,
    Resize,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobInfo {
    pub id: JobId,
    pub cell_id: CellId,
    pub command: CommandSpec,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    pub pid: Option<u32>,
    pub status: ExitStatus,
    #[serde(default)]
    pub termination_reason: Option<TerminationReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Version {},
    Health {},
    CellCreate {
        spec: CellSpec,
    },
    JobRun {
        cell_id: CellId,
        cmd: CommandSpec,
    },
    JobStatus {
        job_id: JobId,
    },
    JobKill {
        job_id: JobId,
        force: bool,
    },
    CellRemove {
        cell_id: CellId,
        force: bool,
    },
    LogsRead {
        job_id: JobId,
        stream: LogStream,
        offset: u64,
        max_bytes: u32,
        follow: bool,
        wait_ms: u64,
    },
    PtyOpen {
        shell: String,
        args: Vec<String>,
        cwd: Option<String>,
        env: BTreeMap<String, String>,
        cols: u16,
        rows: u16,
    },
    PtyInput {
        session_id: SessionId,
        data: Vec<u8>,
    },
    PtyRead {
        session_id: SessionId,
        offset: u64,
        max_bytes: u32,
        follow: bool,
        wait_ms: u64,
    },
    PtyResize {
        session_id: SessionId,
        cols: u16,
        rows: u16,
    },
    PtyClose {
        session_id: SessionId,
        force: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Version {
        daemon: String,
        protocol: u32,
    },
    Health {
        status: String,
    },
    CellCreated {
        cell: CellInfo,
    },
    JobStarted {
        job: JobInfo,
    },
    JobStatus {
        job: JobInfo,
    },
    JobKilled {
        job_id: JobId,
        signal: String,
        status: ExitStatus,
    },
    CellRemoved {
        cell_id: CellId,
    },
    LogsChunk {
        job_id: JobId,
        stream: LogStream,
        offset: u64,
        data: Vec<u8>,
        eof: bool,
        complete: bool,
    },
    PtyOpened {
        session_id: SessionId,
        pid: Option<u32>,
    },
    PtyChunk {
        session_id: SessionId,
        offset: u64,
        data: Vec<u8>,
        eof: bool,
        complete: bool,
        exit_code: Option<i32>,
    },
    PtyAck {
        session_id: SessionId,
        action: PtyAction,
    },
    UsageSample {
        job_id: JobId,
        rss_bytes: Option<u64>,
        cpu_nanos: Option<u64>,
        timestamp_ms: u64,
    },
    Error {
        code: ErrorCode,
        message: String,
        detail: Option<String>,
    },
}
