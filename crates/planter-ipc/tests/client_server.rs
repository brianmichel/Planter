use std::sync::Arc;

use async_trait::async_trait;
use planter_core::{ErrorCode, PROTOCOL_VERSION, Request, Response};
use planter_ipc::{PlanterClient, RequestHandler, serve_unix};
use tempfile::tempdir;
use tokio::time::{Duration, sleep};

struct TestHandler;

#[async_trait]
impl RequestHandler for TestHandler {
    async fn handle(&self, req: Request) -> Response {
        match req {
            Request::Version {} => Response::Version {
                daemon: "0.1.0".to_string(),
                protocol: PROTOCOL_VERSION,
            },
            Request::Health {} => Response::Health {
                status: "ok".to_string(),
            },
            Request::CellCreate { .. } | Request::JobRun { .. } => Response::Error {
                code: ErrorCode::InvalidRequest,
                message: "unsupported in test".to_string(),
                detail: None,
            },
        }
    }
}

#[tokio::test]
async fn client_server_version_and_health_roundtrip() {
    let tmp = tempdir().expect("tempdir should be created");
    let socket_path = tmp.path().join("planterd.sock");

    let handler = Arc::new(TestHandler);
    let server_socket = socket_path.clone();
    let server = tokio::spawn(async move { serve_unix(&server_socket, handler).await });

    let mut client = None;
    for _ in 0..200 {
        match PlanterClient::connect(&socket_path).await {
            Ok(connected) => {
                client = Some(connected);
                break;
            }
            Err(planter_ipc::IpcError::Io(_)) => {
                sleep(Duration::from_millis(10)).await;
            }
            Err(err) => panic!("client should connect: {err}"),
        }
    }
    let mut client = client.expect("client should connect");

    let version = client
        .call(Request::Version {})
        .await
        .expect("version call should succeed");
    match version {
        Response::Version { protocol, .. } => {
            assert_eq!(protocol, PROTOCOL_VERSION);
        }
        other => panic!("unexpected response: {other:?}"),
    }

    let health = client
        .call(Request::Health {})
        .await
        .expect("health call should succeed");
    match health {
        Response::Health { status } => {
            assert_eq!(status, "ok");
        }
        other => panic!("unexpected response: {other:?}"),
    }

    server.abort();
}
