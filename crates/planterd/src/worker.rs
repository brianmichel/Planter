use std::time::{SystemTime, UNIX_EPOCH};

use planter_core::{ErrorCode, PlanterError};
use planter_execd_proto::{
    EXECD_PROTOCOL_VERSION, ExecErrorCode, ExecRequest, ExecRequestEnvelope, ExecResponse,
    ExecResponseEnvelope,
};
use planter_ipc::{
    codec::{decode, encode},
    framing::{read_frame, write_frame},
};
use tokio::net::UnixStream;

pub struct WorkerClient {
    stream: UnixStream,
    next_req_id: u64,
}

impl WorkerClient {
    pub fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            next_req_id: 1,
        }
    }

    pub async fn hello(&mut self, auth_token: String, cell_id: String) -> Result<(), PlanterError> {
        let response = self
            .call(ExecRequest::Hello {
                protocol: EXECD_PROTOCOL_VERSION,
                auth_token,
                cell_id,
            })
            .await?;

        match response {
            ExecResponse::HelloAck { protocol, .. } if protocol == EXECD_PROTOCOL_VERSION => Ok(()),
            ExecResponse::HelloAck { protocol, .. } => Err(PlanterError {
                code: ErrorCode::ProtocolMismatch,
                message: "worker protocol mismatch".to_string(),
                detail: Some(format!("expected={EXECD_PROTOCOL_VERSION} got={protocol}")),
            }),
            other => Err(PlanterError {
                code: ErrorCode::Internal,
                message: "unexpected worker hello response".to_string(),
                detail: Some(format!("{other:?}")),
            }),
        }
    }

    pub async fn ping(&mut self) -> Result<(), PlanterError> {
        let response = self.call(ExecRequest::Ping {}).await?;
        match response {
            ExecResponse::Pong {} => Ok(()),
            other => Err(PlanterError {
                code: ErrorCode::Internal,
                message: "unexpected worker ping response".to_string(),
                detail: Some(format!("{other:?}")),
            }),
        }
    }

    pub async fn call(&mut self, request: ExecRequest) -> Result<ExecResponse, PlanterError> {
        let req_id = self.next_req_id;
        self.next_req_id = self.next_req_id.saturating_add(1);
        let envelope = ExecRequestEnvelope {
            req_id,
            body: request,
        };
        let payload = encode(&envelope).map_err(to_ipc_error)?;
        write_frame(&mut self.stream, &payload)
            .await
            .map_err(to_ipc_error)?;
        let frame = read_frame(&mut self.stream).await.map_err(to_ipc_error)?;
        let response: ExecResponseEnvelope = decode(&frame).map_err(to_ipc_error)?;

        if response.req_id != req_id {
            return Err(PlanterError {
                code: ErrorCode::ProtocolMismatch,
                message: "worker request id mismatch".to_string(),
                detail: Some(format!("expected={req_id} got={}", response.req_id)),
            });
        }

        match response.body {
            ExecResponse::ExecError {
                code,
                message,
                detail,
            } => Err(PlanterError {
                code: map_exec_error(code),
                message,
                detail,
            }),
            body => Ok(body),
        }
    }
}

#[allow(dead_code)]
pub fn make_socket_pair() -> Result<(UnixStream, UnixStream), PlanterError> {
    let (left, right) = std::os::unix::net::UnixStream::pair().map_err(|err| PlanterError {
        code: ErrorCode::Internal,
        message: "create worker control socket pair".to_string(),
        detail: Some(err.to_string()),
    })?;

    left.set_nonblocking(true).map_err(|err| PlanterError {
        code: ErrorCode::Internal,
        message: "set worker control socket nonblocking".to_string(),
        detail: Some(err.to_string()),
    })?;
    right.set_nonblocking(true).map_err(|err| PlanterError {
        code: ErrorCode::Internal,
        message: "set worker control socket nonblocking".to_string(),
        detail: Some(err.to_string()),
    })?;

    let left = UnixStream::from_std(left).map_err(|err| PlanterError {
        code: ErrorCode::Internal,
        message: "convert worker control socket".to_string(),
        detail: Some(err.to_string()),
    })?;
    let right = UnixStream::from_std(right).map_err(|err| PlanterError {
        code: ErrorCode::Internal,
        message: "convert worker control socket".to_string(),
        detail: Some(err.to_string()),
    })?;

    Ok((left, right))
}

#[allow(dead_code)]
pub fn new_auth_token() -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("wkr-{}-{ts}", std::process::id())
}

fn map_exec_error(code: ExecErrorCode) -> ErrorCode {
    match code {
        ExecErrorCode::InvalidRequest => ErrorCode::InvalidRequest,
        ExecErrorCode::NotFound => ErrorCode::NotFound,
        ExecErrorCode::Unauthorized => ErrorCode::Unavailable,
        ExecErrorCode::Unavailable => ErrorCode::Unavailable,
        ExecErrorCode::Unsupported => ErrorCode::InvalidRequest,
        ExecErrorCode::Internal => ErrorCode::Internal,
    }
}

fn to_ipc_error(err: planter_ipc::IpcError) -> PlanterError {
    PlanterError {
        code: ErrorCode::Internal,
        message: "worker ipc".to_string(),
        detail: Some(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{WorkerClient, make_socket_pair, map_exec_error};
    use planter_core::ErrorCode;
    use planter_execd_proto::{
        ExecErrorCode, ExecRequest, ExecRequestEnvelope, ExecResponse, ExecResponseEnvelope,
    };
    use planter_ipc::{
        codec::{decode, encode},
        framing::{read_frame, write_frame},
    };

    async fn fake_server(mut stream: tokio::net::UnixStream) {
        loop {
            let frame = match read_frame(&mut stream).await {
                Ok(frame) => frame,
                Err(_) => return,
            };
            let request: ExecRequestEnvelope = match decode(&frame) {
                Ok(request) => request,
                Err(_) => return,
            };
            let body = match request.body {
                ExecRequest::Hello { .. } => ExecResponse::HelloAck {
                    protocol: planter_execd_proto::EXECD_PROTOCOL_VERSION,
                    worker_pid: 123,
                },
                ExecRequest::Ping {} => ExecResponse::Pong {},
                _ => ExecResponse::ExecError {
                    code: ExecErrorCode::Unsupported,
                    message: "not implemented".to_string(),
                    detail: None,
                },
            };
            let response = ExecResponseEnvelope {
                req_id: request.req_id,
                body,
            };
            let payload = match encode(&response) {
                Ok(payload) => payload,
                Err(_) => return,
            };
            if write_frame(&mut stream, &payload).await.is_err() {
                return;
            }
        }
    }

    #[tokio::test]
    async fn hello_and_ping_roundtrip() {
        let (client_stream, server_stream) = make_socket_pair().expect("socket pair");
        let server = tokio::spawn(async move {
            fake_server(server_stream).await;
        });

        let mut client = WorkerClient::new(client_stream);
        client
            .hello("token".to_string(), "cell-1".to_string())
            .await
            .expect("hello");
        client.ping().await.expect("ping");

        server.abort();
    }

    #[test]
    fn map_exec_error_unavailable() {
        assert_eq!(
            map_exec_error(ExecErrorCode::Unavailable),
            ErrorCode::Unavailable
        );
        assert_eq!(
            map_exec_error(ExecErrorCode::Unauthorized),
            ErrorCode::Unavailable
        );
    }
}
