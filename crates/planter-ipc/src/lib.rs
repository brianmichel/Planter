//! Async CBOR-over-UNIX-socket transport used by planter components.

mod error;

pub mod client;
pub mod codec;
pub mod framing;
pub mod server;

pub use client::PlanterClient;
pub use error::IpcError;
pub use server::{RequestHandler, serve_unix};
