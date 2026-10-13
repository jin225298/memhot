use std::sync::Arc;

use async_trait::async_trait;

use crate::{
    BackendCapabilities, BackendError, BackendEvent, BackendOutcome, PermissionDecision, RunRequest,
};

/// Delivery sink for incremental backend events.
///
/// Events are projections only. Terminal success/failure is exclusively
/// [`BackendOutcome`] returned by [`BackendSession::run`].
pub type BackendEventSink = Arc<dyn Fn(BackendEvent) -> Result<(), BackendError> + Send + Sync>;

/// One live backend conversation/session.
///
/// A single session serializes [`run`](BackendSession::run) calls. Control
/// methods (cancel / permission / input) must not wait on the run execution
/// lock.
#[async_trait]
pub trait BackendSession: Send + Sync {
    fn capabilities(&self) -> BackendCapabilities;

    /// Adapter-private native session reference. Not a product chat session id.
    fn native_session_id(&self) -> Option<String>;

    /// Always returns an explicit terminal outcome. Pre-start unavailability
    /// is `Failed`, not an empty success.
    async fn run(&self, request: RunRequest, sink: BackendEventSink) -> BackendOutcome;

    /// Request cancellation. `Ok` only means the control action was accepted;
    /// the matching `run` must still return `Cancelled` (or another confirmed
    /// terminal outcome).
    async fn cancel(&self, run_id: &str) -> Result<(), BackendError>;

    async fn respond_permission(
        &self,
        request_id: &str,
        decision: PermissionDecision,
    ) -> Result<(), BackendError>;

    async fn respond_input(&self, request_id: &str, response: String) -> Result<(), BackendError>;

    async fn health(&self) -> Result<(), BackendError>;

    async fn close(&self) -> Result<(), BackendError>;
}
