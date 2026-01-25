//! Pod Service Dispatcher - Routes commands to workers.

use super::worker;
use super::{Command, PodData};
use crate::error::PodError;
use crate::proto::PodState;
use crate::services::image::Command as ImageCommand;
use crate::services::task::{Command as TaskCommand, Event as TaskEvent};
use log::{error, info};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::mpsc;

/// Pod Service Dispatcher.
pub struct PodDispatcher {
    command_rx: mpsc::Receiver<Command>,
    task_event_rx: mpsc::Receiver<TaskEvent>,
    image_tx: mpsc::Sender<ImageCommand>,
    task_tx: mpsc::Sender<TaskCommand>,
    pods: HashMap<String, PodData>,
    pods_dir: PathBuf,
}

impl PodDispatcher {
    /// Create a new Pod Dispatcher.
    pub fn new(
        command_rx: mpsc::Receiver<Command>,
        task_event_rx: mpsc::Receiver<TaskEvent>,
        image_tx: mpsc::Sender<ImageCommand>,
        task_tx: mpsc::Sender<TaskCommand>,
        pods_dir: PathBuf,
    ) -> Self {
        Self {
            command_rx,
            task_event_rx,
            image_tx,
            task_tx,
            pods: HashMap::new(),
            pods_dir,
        }
    }

    /// Run the dispatcher loop.
    pub async fn run(mut self) {
        info!("PodDispatcher: Running and waiting for commands");

        loop {
            tokio::select! {
                Some(cmd) = self.command_rx.recv() => {
                    self.handle_command(cmd).await;
                }
                Some(event) = self.task_event_rx.recv() => {
                    self.handle_task_event(event);
                }
                else => {
                    break;
                }
            }
        }

        info!("PodDispatcher: Shutting down");
    }

    async fn handle_command(&mut self, cmd: Command) {
        match cmd {
            Command::Create {
                id,
                name,
                containers,
                responder,
            } => {
                info!("PodDispatcher: Create pod {}", name);

                if self.pods.contains_key(&id) {
                    let _ = responder.send(Err(PodError::InvalidState {
                        expected: "not exists".to_string(),
                        actual: "exists".to_string(),
                    }));
                    return;
                }

                match worker::create_pod(
                    id.clone(),
                    name,
                    containers,
                    &self.image_tx,
                    &self.pods_dir,
                )
                .await
                {
                    Ok(pod) => {
                        let response = pod.clone().into();
                        self.pods.insert(id, pod);
                        let _ = responder.send(Ok(response));
                    }
                    Err(e) => {
                        error!("PodDispatcher: Failed to create pod: {}", e);
                        let _ = responder.send(Err(e));
                    }
                }
            }
            Command::Start { id, responder } => {
                info!("PodDispatcher: Start pod {}", id);

                let pod = match self.pods.get_mut(&id) {
                    Some(pod) => pod,
                    None => {
                        let _ = responder.send(Err(PodError::NotFound(id)));
                        return;
                    }
                };

                if pod.state != PodState::Created && pod.state != PodState::Stopped {
                    let _ = responder.send(Err(PodError::InvalidState {
                        expected: "created or stopped".to_string(),
                        actual: format!("{:?}", pod.state),
                    }));
                    return;
                }

                match worker::start_pod(pod, &self.task_tx).await {
                    Ok(()) => {
                        let response = pod.clone().into();
                        let _ = responder.send(Ok(response));
                    }
                    Err(e) => {
                        error!("PodDispatcher: Failed to start pod: {}", e);
                        let _ = responder.send(Err(e));
                    }
                }
            }
            Command::Stop {
                id,
                timeout_seconds,
                responder,
            } => {
                info!("PodDispatcher: Stop pod {}", id);

                let pod = match self.pods.get_mut(&id) {
                    Some(pod) => pod,
                    None => {
                        let _ = responder.send(Err(PodError::NotFound(id)));
                        return;
                    }
                };

                if pod.state != PodState::Running {
                    let _ = responder.send(Err(PodError::InvalidState {
                        expected: "running".to_string(),
                        actual: format!("{:?}", pod.state),
                    }));
                    return;
                }

                match worker::stop_pod(pod, &self.task_tx, timeout_seconds).await {
                    Ok(()) => {
                        let response = pod.clone().into();
                        let _ = responder.send(Ok(response));
                    }
                    Err(e) => {
                        error!("PodDispatcher: Failed to stop pod: {}", e);
                        let _ = responder.send(Err(e));
                    }
                }
            }
            Command::Delete {
                id,
                force,
                responder,
            } => {
                info!("PodDispatcher: Delete pod {}", id);

                let pod = match self.pods.get_mut(&id) {
                    Some(pod) => pod,
                    None => {
                        let _ = responder.send(Err(PodError::NotFound(id)));
                        return;
                    }
                };

                if pod.state == PodState::Running && !force {
                    let _ = responder.send(Err(PodError::InvalidState {
                        expected: "stopped".to_string(),
                        actual: "running".to_string(),
                    }));
                    return;
                }

                // Stop first if running
                if pod.state == PodState::Running {
                    let _ = worker::stop_pod(pod, &self.task_tx, 10).await;
                }

                match worker::delete_pod(pod, &self.task_tx, &self.pods_dir).await {
                    Ok(()) => {
                        self.pods.remove(&id);
                        let _ = responder.send(Ok(()));
                    }
                    Err(e) => {
                        error!("PodDispatcher: Failed to delete pod: {}", e);
                        let _ = responder.send(Err(e));
                    }
                }
            }
            Command::Get { id, responder } => match self.pods.get(&id) {
                Some(pod) => {
                    let _ = responder.send(Ok(pod.clone().into()));
                }
                None => {
                    let _ = responder.send(Err(PodError::NotFound(id)));
                }
            },
            Command::List { responder } => {
                let pods: Vec<_> = self.pods.values().cloned().map(|p| p.into()).collect();
                let _ = responder.send(pods);
            }
        }
    }

    fn handle_task_event(&mut self, event: TaskEvent) {
        // Find the pod that contains this container
        let container_id = match &event {
            TaskEvent::ContainerStopped { id, .. } => id,
            TaskEvent::ContainerStartFailed { id, .. } => id,
            TaskEvent::ContainerCreated { id, .. } => id,
            TaskEvent::ContainerCreateFailed { id, .. } => id,
            TaskEvent::ContainerStarted { id } => id,
            TaskEvent::ContainerDeleted { id } => id,
        };

        for pod in self.pods.values_mut() {
            if pod.containers.iter().any(|c| c.id == *container_id) {
                worker::handle_container_event(pod, &event);
                break;
            }
        }
    }
}
