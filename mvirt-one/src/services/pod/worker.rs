//! Pod Worker - Performs actual pod operations.

use super::spec::generate_oci_spec;
use super::{ContainerData, PodData};
use crate::error::PodError;
use crate::proto::{ContainerSpec, ContainerState, PodState};
use crate::services::image::{Command as ImageCommand, PullResponse};
use crate::services::task::{Command as TaskCommand, CreateResponse, Event as TaskEvent};
use log::{info, warn};
use std::path::Path;
use tokio::sync::{mpsc, oneshot};

/// Create a pod by pulling images and preparing bundles.
pub async fn create_pod(
    id: String,
    name: String,
    containers: Vec<ContainerSpec>,
    image_tx: &mpsc::Sender<ImageCommand>,
    pods_dir: &Path,
) -> Result<PodData, PodError> {
    info!("Worker: Creating pod {} ({})", name, id);

    let mut container_data = Vec::new();
    let pod_dir = pods_dir.join(&id);

    for spec in containers {
        info!(
            "Worker: Processing container {} with image {}",
            spec.name, spec.image
        );

        // Pull the image
        let (responder, rx) = oneshot::channel();
        let cmd = ImageCommand::Pull {
            image_ref: spec.image.clone(),
            responder,
        };

        if image_tx.send(cmd).await.is_err() {
            return Err(PodError::ContainerFailed {
                container_id: spec.id.clone(),
                error: "Image service unavailable".to_string(),
            });
        }

        let pull_result: Result<PullResponse, _> =
            rx.await.map_err(|_| PodError::ContainerFailed {
                container_id: spec.id.clone(),
                error: "Image service channel closed".to_string(),
            })?;

        let pull_response = pull_result.map_err(|e| PodError::ContainerFailed {
            container_id: spec.id.clone(),
            error: format!("Image pull failed: {}", e),
        })?;

        info!(
            "Worker: Image pulled for container {}: {}",
            spec.name, pull_response.rootfs_path
        );

        // Create bundle directory
        let bundle_path = pod_dir.join(&spec.id);
        tokio::fs::create_dir_all(&bundle_path)
            .await
            .map_err(|e| PodError::ContainerFailed {
                container_id: spec.id.clone(),
                error: format!("Failed to create bundle directory: {}", e),
            })?;

        // Generate OCI spec (use image config for Entrypoint/Cmd if not specified)
        generate_oci_spec(
            &spec,
            &pull_response.rootfs_path,
            &bundle_path,
            &pull_response.config,
        )
        .await
        .map_err(|e| PodError::ContainerFailed {
            container_id: spec.id.clone(),
            error: format!("Failed to generate OCI spec: {}", e),
        })?;

        container_data.push(ContainerData {
            id: spec.id.clone(),
            name: spec.name.clone(),
            image: spec.image.clone(),
            state: ContainerState::Created,
            exit_code: 0,
            bundle_path: bundle_path.to_string_lossy().to_string(),
            pid: None,
            error_message: String::new(),
            spec,
        });
    }

    Ok(PodData {
        id,
        name,
        state: PodState::Created,
        containers: container_data,
        ip_address: String::new(),
        error_message: String::new(),
    })
}

/// Start a pod by creating and starting all containers.
pub async fn start_pod(
    pod: &mut PodData,
    task_tx: &mpsc::Sender<TaskCommand>,
) -> Result<(), PodError> {
    info!("Worker: Starting pod {} ({})", pod.name, pod.id);

    for container in &mut pod.containers {
        info!("Worker: Creating container {}", container.name);

        // Create container
        let (responder, rx) = oneshot::channel();
        let cmd = TaskCommand::Create {
            container_id: container.id.clone(),
            bundle_path: container.bundle_path.clone(),
            responder,
        };

        if task_tx.send(cmd).await.is_err() {
            container.state = ContainerState::Failed;
            container.error_message = "Task service unavailable".to_string();
            return Err(PodError::ContainerFailed {
                container_id: container.id.clone(),
                error: container.error_message.clone(),
            });
        }

        let create_result: Result<CreateResponse, _> =
            rx.await.map_err(|_| PodError::ContainerFailed {
                container_id: container.id.clone(),
                error: "Task service channel closed".to_string(),
            })?;

        let create_response = create_result.map_err(|e| {
            container.state = ContainerState::Failed;
            container.error_message = e.to_string();
            PodError::ContainerFailed {
                container_id: container.id.clone(),
                error: e.to_string(),
            }
        })?;

        container.pid = Some(create_response.pid);
        info!(
            "Worker: Container {} created with PID {}",
            container.name, create_response.pid
        );

        // Start container
        let (responder, rx) = oneshot::channel();
        let cmd = TaskCommand::Start {
            container_id: container.id.clone(),
            pid: create_response.pid,
            responder,
        };

        if task_tx.send(cmd).await.is_err() {
            container.state = ContainerState::Failed;
            container.error_message = "Task service unavailable".to_string();
            return Err(PodError::ContainerFailed {
                container_id: container.id.clone(),
                error: container.error_message.clone(),
            });
        }

        rx.await
            .map_err(|_| PodError::ContainerFailed {
                container_id: container.id.clone(),
                error: "Task service channel closed".to_string(),
            })?
            .map_err(|e| {
                container.state = ContainerState::Failed;
                container.error_message = e.to_string();
                PodError::ContainerFailed {
                    container_id: container.id.clone(),
                    error: e.to_string(),
                }
            })?;

        container.state = ContainerState::Running;
        info!("Worker: Container {} started", container.name);
    }

    pod.state = PodState::Running;
    info!("Worker: Pod {} started successfully", pod.name);
    Ok(())
}

/// Stop a pod by killing all containers.
pub async fn stop_pod(
    pod: &mut PodData,
    task_tx: &mpsc::Sender<TaskCommand>,
    timeout_seconds: u32,
) -> Result<(), PodError> {
    info!(
        "Worker: Stopping pod {} ({}) with timeout {}s",
        pod.name, pod.id, timeout_seconds
    );

    // Send SIGTERM to all containers
    for container in &mut pod.containers {
        if container.state != ContainerState::Running {
            continue;
        }

        let (responder, rx) = oneshot::channel();
        let cmd = TaskCommand::Kill {
            container_id: container.id.clone(),
            signal: 15, // SIGTERM
            responder,
        };

        if task_tx.send(cmd).await.is_err() {
            warn!("Worker: Failed to send kill to container {}", container.id);
            continue;
        }

        let _ = rx.await;
    }

    // Wait for containers to stop (simplified - in practice would poll state)
    tokio::time::sleep(tokio::time::Duration::from_secs(timeout_seconds as u64)).await;

    // Force kill any remaining
    for container in &mut pod.containers {
        if container.state == ContainerState::Running {
            let (responder, rx) = oneshot::channel();
            let cmd = TaskCommand::Kill {
                container_id: container.id.clone(),
                signal: 9, // SIGKILL
                responder,
            };

            if task_tx.send(cmd).await.is_ok() {
                let _ = rx.await;
            }
        }
        container.state = ContainerState::Stopped;
    }

    pod.state = PodState::Stopped;
    info!("Worker: Pod {} stopped", pod.name);
    Ok(())
}

/// Delete a pod by removing all containers and cleaning up.
pub async fn delete_pod(
    pod: &mut PodData,
    task_tx: &mpsc::Sender<TaskCommand>,
    pods_dir: &Path,
) -> Result<(), PodError> {
    info!("Worker: Deleting pod {} ({})", pod.name, pod.id);

    // Delete all containers
    for container in &pod.containers {
        let (responder, rx) = oneshot::channel();
        let cmd = TaskCommand::Delete {
            container_id: container.id.clone(),
            responder,
        };

        if task_tx.send(cmd).await.is_ok() {
            let _ = rx.await;
        }
    }

    // Remove pod directory
    let pod_dir = pods_dir.join(&pod.id);
    if pod_dir.exists()
        && let Err(e) = tokio::fs::remove_dir_all(&pod_dir).await
    {
        warn!("Worker: Failed to remove pod directory: {}", e);
    }

    info!("Worker: Pod {} deleted", pod.name);
    Ok(())
}

/// Handle container events from the Task Service.
pub fn handle_container_event(pod: &mut PodData, event: &TaskEvent) {
    match event {
        TaskEvent::ContainerStopped { id, exit_code } => {
            if let Some(container) = pod.containers.iter_mut().find(|c| c.id == *id) {
                container.state = ContainerState::Stopped;
                container.exit_code = *exit_code;
                info!(
                    "Container {} stopped with exit code {}",
                    container.name, exit_code
                );

                // Check if all containers are stopped
                let all_stopped = pod
                    .containers
                    .iter()
                    .all(|c| c.state == ContainerState::Stopped);
                if all_stopped {
                    pod.state = PodState::Stopped;
                }
            }
        }
        TaskEvent::ContainerStartFailed { id, error } => {
            if let Some(container) = pod.containers.iter_mut().find(|c| c.id == *id) {
                container.state = ContainerState::Failed;
                container.error_message = error.clone();
                pod.state = PodState::Failed;
                pod.error_message = format!("Container {} failed: {}", container.name, error);
            }
        }
        _ => {}
    }
}
