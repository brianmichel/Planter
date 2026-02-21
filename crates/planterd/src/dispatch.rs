use std::sync::Arc;

use async_trait::async_trait;
use planter_core::{Request, Response};
use planter_ipc::RequestHandler;

use crate::handlers::Handler;

pub struct DaemonDispatcher {
    handler: Handler,
}

impl DaemonDispatcher {
    pub fn new(handler: Handler) -> Self {
        Self { handler }
    }
}

#[async_trait]
impl RequestHandler for DaemonDispatcher {
    async fn handle(&self, req: Request) -> Response {
        self.handler.handle(req).await
    }
}

impl From<Arc<crate::state::StateStore>> for DaemonDispatcher {
    fn from(state: Arc<crate::state::StateStore>) -> Self {
        Self::new(Handler::new(state))
    }
}
