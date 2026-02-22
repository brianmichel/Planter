//! Shared core protocol types and helpers used by all planter binaries.

pub mod errors;
pub mod ids;
pub mod paths;
pub mod protocol;
pub mod time;

pub use errors::{ErrorCode, PlanterError};
pub use ids::{CellId, JobId, ReqId, SessionId};
pub use paths::default_state_dir;
pub use protocol::{
    CellInfo, CellSpec, CommandSpec, ExitStatus, JobInfo, LogStream, PROTOCOL_VERSION, PtyAction,
    Request, RequestEnvelope, ResourceLimits, Response, ResponseEnvelope, TerminationReason,
};
pub use time::now_ms;
