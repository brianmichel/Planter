use std::{path::Path, time::Duration};

use planter_core::{ReqId, Request, RequestEnvelope, Response, ResponseEnvelope};
use tokio::{net::UnixStream, time::timeout};

use crate::{
    IpcError,
    codec::{decode, encode},
    framing::{read_frame, write_frame},
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Stateful UNIX socket client for request/response planter IPC.
pub struct PlanterClient {
    /// Connected socket stream.
    stream: UnixStream,
    /// Next request id to assign.
    next_req_id: u64,
    /// Per-call timeout.
    timeout: Duration,
}

impl PlanterClient {
    /// Connects a client to the daemon socket path.
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self, IpcError> {
        let stream = UnixStream::connect(path).await?;
        Ok(Self {
            stream,
            next_req_id: 1,
            timeout: DEFAULT_TIMEOUT,
        })
    }

    /// Overrides the default call timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sends one request and waits for the matching response.
    pub async fn call(&mut self, req: Request) -> Result<Response, IpcError> {
        let req_id = ReqId(self.next_req_id);
        self.next_req_id = self.next_req_id.saturating_add(1);

        let envelope = RequestEnvelope { req_id, body: req };
        let payload = encode(&envelope)?;

        let response = timeout(self.timeout, async {
            write_frame(&mut self.stream, &payload).await?;
            let response_frame = read_frame(&mut self.stream).await?;
            decode::<ResponseEnvelope<Response>>(&response_frame)
        })
        .await
        .map_err(|_| IpcError::Timeout)??;

        if response.req_id != req_id {
            return Err(IpcError::RequestIdMismatch {
                expected: req_id.0,
                actual: response.req_id.0,
            });
        }

        Ok(response.body)
    }
}
