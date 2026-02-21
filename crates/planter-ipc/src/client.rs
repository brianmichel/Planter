use std::{path::Path, time::Duration};

use planter_core::{ReqId, Request, RequestEnvelope, Response, ResponseEnvelope};
use tokio::{net::UnixStream, time::timeout};

use crate::{
    IpcError,
    codec::{decode, encode},
    framing::{read_frame, write_frame},
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

pub struct PlanterClient {
    stream: UnixStream,
    next_req_id: u64,
    timeout: Duration,
}

impl PlanterClient {
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self, IpcError> {
        let stream = UnixStream::connect(path).await?;
        Ok(Self {
            stream,
            next_req_id: 1,
            timeout: DEFAULT_TIMEOUT,
        })
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

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
