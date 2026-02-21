use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{CellId, ErrorCode, JobId, ReqId};

pub const PROTOCOL_VERSION: u32 = 1;

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
pub struct CommandSpec {
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellInfo {
    pub id: CellId,
    pub spec: CellSpec,
    pub created_at_ms: u64,
    pub dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExitStatus {
    Running,
    Exited { code: Option<i32> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobInfo {
    pub id: JobId,
    pub cell_id: CellId,
    pub command: CommandSpec,
    pub stdout_path: String,
    pub stderr_path: String,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    pub pid: Option<u32>,
    pub status: ExitStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Version {},
    Health {},
    CellCreate { spec: CellSpec },
    JobRun { cell_id: CellId, cmd: CommandSpec },
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
    Error {
        code: ErrorCode,
        message: String,
        detail: Option<String>,
    },
}
