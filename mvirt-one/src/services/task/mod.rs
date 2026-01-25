//! Task Service - Low-level OCI runtime interface.
//!
//! Wraps youki commands and manages container lifecycle at the runtime level.
//! Based on FeOS task-service pattern.

mod dispatcher;
mod worker;

pub use dispatcher::TaskDispatcher;

use crate::error::ContainerError;
use tokio::sync::oneshot;

/// Commands that can be sent to the Task Service.
#[derive(Debug)]
pub enum Command {
    Create {
        container_id: String,
        bundle_path: String,
        responder: oneshot::Sender<Result<CreateResponse, ContainerError>>,
    },
    Start {
        container_id: String,
        pid: i32,
        responder: oneshot::Sender<Result<(), ContainerError>>,
    },
    Kill {
        container_id: String,
        signal: i32,
        responder: oneshot::Sender<Result<(), ContainerError>>,
    },
    Delete {
        container_id: String,
        responder: oneshot::Sender<Result<(), ContainerError>>,
    },
}

/// Response from container creation.
#[derive(Debug)]
pub struct CreateResponse {
    pub pid: i32,
}

/// Events emitted by the Task Service.
#[derive(Debug, Clone)]
pub enum Event {
    ContainerCreated { id: String, pid: i32 },
    ContainerCreateFailed { id: String, error: String },
    ContainerStarted { id: String },
    ContainerStartFailed { id: String, error: String },
    ContainerStopped { id: String, exit_code: i32 },
    ContainerDeleted { id: String },
}
