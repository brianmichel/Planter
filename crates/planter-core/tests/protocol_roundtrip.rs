use planter_core::{
    ErrorCode, PROTOCOL_VERSION, ReqId, Request, RequestEnvelope, Response, ResponseEnvelope,
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
}
