use std::collections::BTreeMap;

use planter_core::{
    CellId, CellSpec, CommandSpec, ErrorCode, PROTOCOL_VERSION, ReqId, Request, RequestEnvelope,
    Response, ResponseEnvelope,
};

#[test]
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
}

#[test]
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
                },
                stdout_path: "/tmp/stdout.log".to_string(),
                stderr_path: "/tmp/stderr.log".to_string(),
                started_at_ms: 1,
                finished_at_ms: None,
                pid: Some(100),
                status: planter_core::ExitStatus::Running,
            },
        },
    };

    let encoded = serde_cbor::to_vec(&run_response).expect("response encode should succeed");
    let decoded: ResponseEnvelope<Response> =
        serde_cbor::from_slice(&encoded).expect("response decode should succeed");
    assert_eq!(decoded, run_response);
}
