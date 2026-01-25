//! Task Service Worker - Executes youki commands.
//!
//! Based on FeOS task-service/worker.rs pattern.

use super::{CreateResponse, Event};
use crate::error::ContainerError;
use log::{debug, error, info, warn};
use nix::sys::wait::{WaitStatus, waitpid};
use nix::unistd::Pid;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};

/// Run a short-lived youki command.
async fn run_youki_command(youki_path: &PathBuf, args: &[&str]) -> Result<(), ContainerError> {
    info!(
        "Worker: Executing youki {} {}",
        youki_path.display(),
        args.join(" ")
    );

    let output = Command::new(youki_path)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| ContainerError::YoukiCommand(format!("Failed to execute youki: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let err_msg = format!(
            "youki exited with code {}: stderr='{}', stdout='{}'",
            output.status, stderr, stdout
        );
        error!("Worker: {err_msg}");
        return Err(ContainerError::YoukiCommand(err_msg));
    }

    debug!("Worker: youki command successful");
    Ok(())
}

/// Handle container creation.
pub async fn handle_create(
    container_id: String,
    bundle_path: String,
    event_tx: mpsc::Sender<Event>,
    responder: oneshot::Sender<Result<CreateResponse, ContainerError>>,
    youki_path: Arc<PathBuf>,
) {
    let id = container_id.clone();
    let pid_file = format!("{}/container.pid", bundle_path);

    let args = &[
        "create",
        "--bundle",
        &bundle_path,
        "--pid-file",
        &pid_file,
        &id,
    ];

    info!(
        "Worker: Spawning youki create: {} {}",
        youki_path.display(),
        args.join(" ")
    );

    let child_result = Command::new(youki_path.as_ref())
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn();

    let mut child = match child_result {
        Ok(child) => child,
        Err(e) => {
            let err = ContainerError::YoukiCommand(format!("Failed to spawn youki create: {e}"));
            let _ = event_tx
                .send(Event::ContainerCreateFailed {
                    id,
                    error: err.to_string(),
                })
                .await;
            let _ = responder.send(Err(err));
            return;
        }
    };

    let status = match child.wait().await {
        Ok(status) => status,
        Err(e) => {
            let err = ContainerError::YoukiCommand(format!("Failed to wait for youki create: {e}"));
            let _ = event_tx
                .send(Event::ContainerCreateFailed {
                    id,
                    error: err.to_string(),
                })
                .await;
            let _ = responder.send(Err(err));
            return;
        }
    };

    if !status.success() {
        let err =
            ContainerError::YoukiCommand(format!("youki create exited with non-zero: {status}"));
        let _ = event_tx
            .send(Event::ContainerCreateFailed {
                id,
                error: err.to_string(),
            })
            .await;
        let _ = responder.send(Err(err));
        return;
    }

    // Read PID from file
    let result: Result<i32, ContainerError> = async {
        let pid_str = tokio::fs::read_to_string(&pid_file)
            .await
            .map_err(|e| ContainerError::YoukiCommand(format!("Could not read pid file: {e}")))?;
        let pid = pid_str
            .trim()
            .parse::<i32>()
            .map_err(|e| ContainerError::YoukiCommand(format!("Failed to parse PID: {e}")))?;
        let _ = tokio::fs::remove_file(&pid_file).await;
        Ok(pid)
    }
    .await;

    match result {
        Ok(pid) => {
            info!("Worker: Container {} created with PID {}", id, pid);
            let _ = event_tx.send(Event::ContainerCreated { id, pid }).await;
            let _ = responder.send(Ok(CreateResponse { pid }));
        }
        Err(e) => {
            let _ = event_tx
                .send(Event::ContainerCreateFailed {
                    id,
                    error: e.to_string(),
                })
                .await;
            let _ = responder.send(Err(e));
        }
    }
}

/// Handle container start.
pub async fn handle_start(
    container_id: String,
    pid: i32,
    event_tx: mpsc::Sender<Event>,
    responder: oneshot::Sender<Result<(), ContainerError>>,
    youki_path: Arc<PathBuf>,
) {
    let id = container_id.clone();
    let result = run_youki_command(&youki_path, &["start", &id]).await;

    match result {
        Ok(_) => {
            let _ = event_tx
                .send(Event::ContainerStarted { id: id.clone() })
                .await;
            let _ = responder.send(Ok(()));

            // Spawn background task to wait for container exit
            tokio::spawn(wait_for_process_exit(id, pid, event_tx));
        }
        Err(e) => {
            let _ = event_tx
                .send(Event::ContainerStartFailed {
                    id,
                    error: e.to_string(),
                })
                .await;
            let _ = responder.send(Err(e));
        }
    }
}

/// Handle container kill.
pub async fn handle_kill(
    container_id: String,
    signal: i32,
    responder: oneshot::Sender<Result<(), ContainerError>>,
    youki_path: Arc<PathBuf>,
) {
    let signal_str = signal.to_string();
    let result = run_youki_command(&youki_path, &["kill", &container_id, &signal_str]).await;
    let _ = responder.send(result);
}

/// Handle container deletion.
pub async fn handle_delete(
    container_id: String,
    event_tx: mpsc::Sender<Event>,
    responder: oneshot::Sender<Result<(), ContainerError>>,
    youki_path: Arc<PathBuf>,
) {
    let id = container_id.clone();
    let result = run_youki_command(&youki_path, &["delete", "--force", &id]).await;

    if let Err(e) = result {
        let _ = responder.send(Err(e));
        return;
    }

    let _ = event_tx.send(Event::ContainerDeleted { id }).await;
    let _ = responder.send(Ok(()));
}

/// Wait for a container process to exit and send an event.
async fn wait_for_process_exit(id: String, pid: i32, event_tx: mpsc::Sender<Event>) {
    info!("Worker: Waiting for container {} (PID {}) to exit", id, pid);
    let pid_obj = Pid::from_raw(pid);

    // Use spawn_blocking since waitpid is a blocking syscall
    let wait_result = tokio::task::spawn_blocking(move || waitpid(pid_obj, None))
        .await
        .unwrap_or(Err(nix::errno::Errno::ECHILD));

    let exit_code = match wait_result {
        Ok(WaitStatus::Exited(_, code)) => {
            info!("Worker: Container {} exited with code {}", id, code);
            code
        }
        Ok(WaitStatus::Signaled(_, signal, _)) => {
            info!(
                "Worker: Container {} was terminated by signal {:?}",
                id, signal
            );
            128 + (signal as i32)
        }
        Ok(status) => {
            warn!(
                "Worker: Container {} ended with unexpected status: {:?}",
                id, status
            );
            255
        }
        Err(e) => {
            error!("Worker: waitpid failed for container {}: {}", id, e);
            255
        }
    };

    if event_tx
        .send(Event::ContainerStopped { id, exit_code })
        .await
        .is_err()
    {
        error!("Worker: Failed to send ContainerStopped event");
    }
}
