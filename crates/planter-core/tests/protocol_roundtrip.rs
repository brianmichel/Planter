use std::collections::BTreeMap;

use planter_core::{
    CellId, CellSpec, CommandSpec, ErrorCode, LogStream, PROTOCOL_VERSION, ReqId, Request,
    RequestEnvelope, ResourceLimits, Response, ResponseEnvelope, SessionId,
};

#[test]
/// Verifies representative request envelopes roundtrip through CBOR encoding.
fn request_envelope_roundtrip_cbor() {
    let input = RequestEnvelope {
        req_id: ReqId(42),
        body: Request::Version {},
    };

    let encoded = serde_cbor::to_vec(&input).expect("request encode should succeed");
    let decoded: RequestEnvelope<Request> =
        serde_cbor::from_slice(&encoded).expect("request decode should succeed");

    assert_eq!(decoded, input);

    let create_request = RequestEnvelope {
        req_id: ReqId(43),
        body: Request::CellCreate {
            spec: CellSpec {
                name: "demo".to_string(),
                env: BTreeMap::from([(String::from("FOO"), String::from("bar"))]),
            },
        },
    };

    let encoded = serde_cbor::to_vec(&create_request).expect("request encode should succeed");
    let decoded: RequestEnvelope<Request> =
        serde_cbor::from_slice(&encoded).expect("request decode should succeed");
    assert_eq!(decoded, create_request);

    let logs_request = RequestEnvelope {
        req_id: ReqId(44),
        body: Request::LogsRead {
            job_id: planter_core::JobId("job-1".to_string()),
            stream: LogStream::Stdout,
            offset: 0,
            max_bytes: 1024,
            follow: true,
            wait_ms: 500,
        },
    };
    let encoded = serde_cbor::to_vec(&logs_request).expect("request encode should succeed");
    let decoded: RequestEnvelope<Request> =
        serde_cbor::from_slice(&encoded).expect("request decode should succeed");
    assert_eq!(decoded, logs_request);

    let pty_request = RequestEnvelope {
        req_id: ReqId(45),
        body: Request::PtyRead {
            session_id: SessionId(7),
            offset: 128,
            max_bytes: 2048,
            follow: true,
            wait_ms: 250,
        },
    };
    let encoded = serde_cbor::to_vec(&pty_request).expect("request encode should succeed");
    let decoded: RequestEnvelope<Request> =
        serde_cbor::from_slice(&encoded).expect("request decode should succeed");
    assert_eq!(decoded, pty_request);
}

#[test]
/// Verifies representative response envelopes roundtrip through CBOR encoding.
fn response_envelope_roundtrip_cbor() {
    let version = ResponseEnvelope {
        req_id: ReqId(1),
        body: Response::Version {
            daemon: "0.1.0".to_string(),
            protocol: PROTOCOL_VERSION,
        },
    };

    let encoded = serde_cbor::to_vec(&version).expect("response encode should succeed");
    let decoded: ResponseEnvelope<Response> =
        serde_cbor::from_slice(&encoded).expect("response decode should succeed");

    assert_eq!(decoded, version);

    let error = ResponseEnvelope {
        req_id: ReqId(2),
        body: Response::Error {
            code: ErrorCode::InvalidRequest,
            message: "bad request".to_string(),
            detail: Some("missing field body".to_string()),
        },
    };

    let encoded = serde_cbor::to_vec(&error).expect("error encode should succeed");
    let decoded: ResponseEnvelope<Response> =
        serde_cbor::from_slice(&encoded).expect("error decode should succeed");

    assert_eq!(decoded, error);

    let run_response = ResponseEnvelope {
        req_id: ReqId(3),
        body: Response::JobStarted {
            job: planter_core::JobInfo {
                id: planter_core::JobId("job-1".to_string()),
                cell_id: CellId("cell-1".to_string()),
                command: CommandSpec {
                    argv: vec!["echo".to_string(), "ok".to_string()],
                    cwd: None,
                    env: BTreeMap::new(),
                    limits: Some(ResourceLimits {
                        timeout_ms: Some(1000),
                        max_rss_bytes: None,
                        max_log_bytes: None,
                    }),
                },
                started_at_ms: 1,
                finished_at_ms: None,
                pid: Some(100),
                status: planter_core::ExitStatus::Running,
                termination_reason: None,
            },
        },
    };

    let encoded = serde_cbor::to_vec(&run_response).expect("response encode should succeed");
    let decoded: ResponseEnvelope<Response> =
        serde_cbor::from_slice(&encoded).expect("response decode should succeed");
    assert_eq!(decoded, run_response);

    let logs = ResponseEnvelope {
        req_id: ReqId(4),
        body: Response::LogsChunk {
            job_id: planter_core::JobId("job-1".to_string()),
            stream: LogStream::Stdout,
            offset: 0,
            data: b"hello".to_vec(),
            eof: true,
            complete: true,
        },
    };

    let encoded = serde_cbor::to_vec(&logs).expect("response encode should succeed");
    let decoded: ResponseEnvelope<Response> =
        serde_cbor::from_slice(&encoded).expect("response decode should succeed");
    assert_eq!(decoded, logs);

    let pty = ResponseEnvelope {
        req_id: ReqId(5),
        body: Response::PtyChunk {
            session_id: SessionId(7),
            offset: 128,
            data: b"shell".to_vec(),
            eof: false,
            complete: false,
            exit_code: None,
        },
    };

    let encoded = serde_cbor::to_vec(&pty).expect("response encode should succeed");
    let decoded: ResponseEnvelope<Response> =
        serde_cbor::from_slice(&encoded).expect("response decode should succeed");
    assert_eq!(decoded, pty);
}
