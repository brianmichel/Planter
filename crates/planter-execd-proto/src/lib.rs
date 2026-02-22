use std::collections::BTreeMap;

use planter_core::{CommandSpec, ErrorCode, ExitStatus, JobId, SessionId, TerminationReason};
use serde::{Deserialize, Serialize};

pub const EXECD_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecRequestEnvelope {
    pub req_id: u64,
    pub body: ExecRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecResponseEnvelope {
    pub req_id: u64,
    pub body: ExecResponse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecPtyAction {
    Opened,
    Input,
    Resize,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecErrorCode {
    InvalidRequest,
    NotFound,
    Unauthorized,
    Unavailable,
    Unsupported,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecRequest {
    Hello {
        protocol: u32,
        auth_token: String,
        cell_id: String,
    },
    Ping {},
    RunJob {
        job_id: JobId,
        cmd: CommandSpec,
        env: BTreeMap<String, String>,
        stdout_path: String,
        stderr_path: String,
    },
    JobStatus {
        job_id: JobId,
    },
    JobSignal {
        job_id: JobId,
        force: bool,
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
    UsageProbe {
        job_id: JobId,
    },
    Shutdown {
        force: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecResponse {
    HelloAck {
        protocol: u32,
        worker_pid: u32,
    },
    Pong {},
    JobStarted {
        job_id: JobId,
        pid: Option<u32>,
    },
    JobStatus {
        job_id: JobId,
        status: ExitStatus,
        finished_at_ms: Option<u64>,
        termination_reason: Option<TerminationReason>,
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
        action: ExecPtyAction,
    },
    UsageSample {
        job_id: JobId,
        rss_bytes: Option<u64>,
        cpu_nanos: Option<u64>,
        timestamp_ms: u64,
    },
    ExecError {
        code: ExecErrorCode,
        message: String,
        detail: Option<String>,
    },
}

impl From<ErrorCode> for ExecErrorCode {
    fn from(value: ErrorCode) -> Self {
        match value {
            ErrorCode::InvalidRequest => ExecErrorCode::InvalidRequest,
            ErrorCode::NotFound => ExecErrorCode::NotFound,
            ErrorCode::Timeout => ExecErrorCode::Unavailable,
            ErrorCode::ProtocolMismatch => ExecErrorCode::InvalidRequest,
            ErrorCode::Unavailable => ExecErrorCode::Unavailable,
            ErrorCode::Internal => ExecErrorCode::Internal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EXECD_PROTOCOL_VERSION, ExecRequest, ExecRequestEnvelope, ExecResponse,
        ExecResponseEnvelope,
    };
    use planter_core::CommandSpec;
    use std::collections::BTreeMap;

    #[test]
    fn roundtrip_hello_request() {
        let request = ExecRequestEnvelope {
            req_id: 42,
            body: ExecRequest::Hello {
                protocol: EXECD_PROTOCOL_VERSION,
                auth_token: "abc123".to_string(),
                cell_id: "cell-1".to_string(),
            },
        };
        let bytes = serde_cbor::to_vec(&request).expect("encode request");
        let decoded =
            serde_cbor::from_slice::<ExecRequestEnvelope>(&bytes).expect("decode request");
        assert_eq!(decoded, request);
    }

    #[test]
    fn roundtrip_run_job_request() {
        let request = ExecRequestEnvelope {
            req_id: 7,
            body: ExecRequest::RunJob {
                job_id: planter_core::JobId("job-1".to_string()),
                cmd: CommandSpec {
                    argv: vec![
                        "/bin/sh".to_string(),
                        "-c".to_string(),
                        "echo hi".to_string(),
                    ],
                    cwd: None,
                    env: BTreeMap::new(),
                    limits: None,
                },
                env: BTreeMap::new(),
                stdout_path: "/tmp/stdout.log".to_string(),
                stderr_path: "/tmp/stderr.log".to_string(),
            },
        };
        let bytes = serde_cbor::to_vec(&request).expect("encode request");
        let decoded =
            serde_cbor::from_slice::<ExecRequestEnvelope>(&bytes).expect("decode request");
        assert_eq!(decoded, request);
    }

    #[test]
    fn roundtrip_response() {
        let response = ExecResponseEnvelope {
            req_id: 7,
            body: ExecResponse::Pong {},
        };
        let bytes = serde_cbor::to_vec(&response).expect("encode response");
        let decoded =
            serde_cbor::from_slice::<ExecResponseEnvelope>(&bytes).expect("decode response");
        assert_eq!(decoded, response);
    }
}
