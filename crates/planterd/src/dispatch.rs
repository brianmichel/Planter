use std::sync::Arc;

use async_trait::async_trait;
use planter_core::{Request, Response};
use planter_ipc::RequestHandler;

use crate::handlers::Handler;

/// Adapts daemon request handling to the generic IPC request-handler trait.
pub struct DaemonDispatcher {
    /// Request handler containing daemon business logic.
    handler: Handler,
}

impl DaemonDispatcher {
    /// Creates a dispatcher from a concrete handler.
    pub fn new(handler: Handler) -> Self {
        Self { handler }
    }
}

#[async_trait]
impl RequestHandler for DaemonDispatcher {
    /// Routes one request through the daemon handler.
    async fn handle(&self, req: Request) -> Response {
        self.handler.handle(req).await
    }
}

impl From<Arc<crate::state::StateStore>> for DaemonDispatcher {
    /// Builds a dispatcher backed by a state store.
    fn from(state: Arc<crate::state::StateStore>) -> Self {
        Self::new(Handler::new(state))
    }
}
