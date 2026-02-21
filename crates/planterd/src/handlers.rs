use planter_core::{PROTOCOL_VERSION, Request, Response};

pub fn handle(request: Request) -> Response {
    match request {
        Request::Version {} => Response::Version {
            daemon: env!("CARGO_PKG_VERSION").to_string(),
            protocol: PROTOCOL_VERSION,
        },
        Request::Health {} => Response::Health {
            status: "ok".to_string(),
        },
    }
}
