//! Pod Service - High-level pod and container lifecycle management.
//!
//! Coordinates Image Service and Task Service to manage pods.
//! Based on FeOS container-service pattern.

mod api;
mod dispatcher;
mod spec;
mod worker;

pub use api::PodApiHandler;
pub use dispatcher::PodDispatcher;

use crate::error::PodError;
use crate::proto::{Container, ContainerSpec, ContainerState, Pod, PodState};
use tokio::sync::oneshot;

/// Commands that can be sent to the Pod Service.
#[derive(Debug)]
pub enum Command {
    Create {
        id: String,
        name: String,
        containers: Vec<ContainerSpec>,
        responder: oneshot::Sender<Result<Pod, PodError>>,
    },
    Start {
        id: String,
        responder: oneshot::Sender<Result<Pod, PodError>>,
    },
    Stop {
        id: String,
        timeout_seconds: u32,
        responder: oneshot::Sender<Result<Pod, PodError>>,
    },
    Delete {
        id: String,
        force: bool,
        responder: oneshot::Sender<Result<(), PodError>>,
    },
    Get {
        id: String,
        responder: oneshot::Sender<Result<Pod, PodError>>,
    },
    List {
        responder: oneshot::Sender<Vec<Pod>>,
    },
}

/// Internal pod state.
#[derive(Debug, Clone)]
pub struct PodData {
    pub id: String,
    pub name: String,
    pub state: PodState,
    pub containers: Vec<ContainerData>,
    pub ip_address: String,
    pub error_message: String,
}

/// Internal container state.
#[derive(Debug, Clone)]
pub struct ContainerData {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: ContainerState,
    pub exit_code: i32,
    pub bundle_path: String,
    pub pid: Option<i32>,
    pub error_message: String,
    pub spec: ContainerSpec,
}

impl From<PodData> for Pod {
    fn from(data: PodData) -> Self {
        Pod {
            id: data.id,
            name: data.name,
            state: data.state.into(),
            containers: data.containers.into_iter().map(|c| c.into()).collect(),
            ip_address: data.ip_address,
            error_message: data.error_message,
        }
    }
}

impl From<ContainerData> for Container {
    fn from(data: ContainerData) -> Self {
        Container {
            id: data.id,
            name: data.name,
            state: data.state.into(),
            exit_code: data.exit_code,
            image: data.image,
            error_message: data.error_message,
        }
    }
}
