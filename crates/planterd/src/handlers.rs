use std::sync::Arc;

use planter_core::{PROTOCOL_VERSION, PlanterError, Request, Response};

use crate::state::StateStore;

#[derive(Clone)]
pub struct Handler {
    state: Arc<StateStore>,
}

impl Handler {
    pub fn new(state: Arc<StateStore>) -> Self {
        Self { state }
    }

    pub async fn handle(&self, request: Request) -> Response {
        let result = match request {
            Request::Version {} => Ok(Response::Version {
                daemon: env!("CARGO_PKG_VERSION").to_string(),
                protocol: PROTOCOL_VERSION,
            }),
            Request::Health {} => Ok(Response::Health {
                status: "ok".to_string(),
            }),
            Request::CellCreate { spec } => self
                .state
                .create_cell(spec)
                .map(|cell| Response::CellCreated { cell }),
            Request::JobRun { cell_id, cmd } => self
                .state
                .run_job(cell_id, cmd)
                .await
                .map(|job| Response::JobStarted { job }),
        };

        match result {
            Ok(response) => response,
            Err(err) => to_error_response(err),
        }
    }
}

fn to_error_response(err: PlanterError) -> Response {
    Response::Error {
        code: err.code,
        message: err.message,
        detail: err.detail,
    }
}
