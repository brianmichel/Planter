use async_trait::async_trait;
use planter_core::{Request, Response};
use planter_ipc::RequestHandler;

pub struct DaemonDispatcher;

#[async_trait]
impl RequestHandler for DaemonDispatcher {
    async fn handle(&self, req: Request) -> Response {
        crate::handlers::handle(req)
    }
}
