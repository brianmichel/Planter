use std::{io::ErrorKind, path::Path, sync::Arc};

use async_trait::async_trait;
use planter_core::{ErrorCode, ReqId, Request, RequestEnvelope, Response, ResponseEnvelope};
use serde::Deserialize;
use tokio::net::{UnixListener, UnixStream};

use crate::{
    IpcError,
    codec::{decode, encode},
    framing::{read_frame, write_frame},
};

/// Async request handler used by the IPC server loop.
#[async_trait]
pub trait RequestHandler: Send + Sync + 'static {
    /// Handles one decoded request and returns a response payload.
    async fn handle(&self, req: Request) -> Response;
}

/// Serves the planter IPC protocol over a UNIX domain socket.
pub async fn serve_unix(path: &Path, handler: Arc<dyn RequestHandler>) -> Result<(), IpcError> {
    let listener = UnixListener::bind(path)?;

    loop {
        let (stream, _) = listener.accept().await?;
        let handler = Arc::clone(&handler);

        tokio::spawn(async move {
            if let Err(err) = handle_connection(stream, handler).await {
                tracing::debug!(error = %err, "connection handler exited with error");
            }
        });
    }
}

/// Handles request/response framing for a single accepted connection.
async fn handle_connection(
    mut stream: UnixStream,
    handler: Arc<dyn RequestHandler>,
) -> Result<(), IpcError> {
    loop {
        let frame = match read_frame(&mut stream).await {
            Ok(frame) => frame,
            Err(IpcError::Io(err))
                if matches!(
                    err.kind(),
                    ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset | ErrorKind::BrokenPipe
                ) =>
            {
                return Ok(());
            }
            Err(err) => return Err(err),
        };

        match decode::<RequestEnvelope<Request>>(&frame) {
            Ok(req) => {
                let response = handler.handle(req.body).await;
                let envelope = ResponseEnvelope {
                    req_id: req.req_id,
                    body: response,
                };
                let payload = encode(&envelope)?;
                write_frame(&mut stream, &payload).await?;
            }
            Err(err) => {
                if let Some(req_id) = extract_req_id(&frame) {
                    let envelope = ResponseEnvelope {
                        req_id,
                        body: Response::Error {
                            code: ErrorCode::InvalidRequest,
                            message: "failed to decode request envelope".to_string(),
                            detail: Some(err.to_string()),
                        },
                    };
                    let payload = encode(&envelope)?;
                    let _ = write_frame(&mut stream, &payload).await;
                }

                return Ok(());
            }
        }
    }
}

/// Minimal decode target used to recover `req_id` from malformed requests.
#[derive(Debug, Deserialize)]
struct ReqIdOnly {
    /// Request identifier field from the envelope.
    req_id: ReqId,
}

/// Extracts a request id from a partially valid request envelope frame.
fn extract_req_id(frame: &[u8]) -> Option<ReqId> {
    decode::<ReqIdOnly>(frame)
        .ok()
        .map(|decoded| decoded.req_id)
}
