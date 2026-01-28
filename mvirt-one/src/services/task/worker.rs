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
///
/// If `youki_root` is Some, prepends `--root <path>` to the command.
/// This allows using a custom root directory for youki container state.
async fn run_youki_command(
    youki_path: &PathBuf,
    youki_root: Option<&PathBuf>,
    args: &[&str],
) -> Result<String, ContainerError> {
    // Build full args as owned strings: optionally prepend --root <path>
    let mut full_args: Vec<String> = Vec::new();
    if let Some(root) = youki_root {
        full_args.push("--root".to_string());
        full_args.push(root.to_string_lossy().to_string());
    }
    full_args.extend(args.iter().map(|s| s.to_string()));

    info!(
        "Worker: Executing youki {} {}",
        youki_path.display(),
        full_args.join(" ")
    );

    let output = Command::new(youki_path)
        .args(&full_args)
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
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(stdout)
}

/// Handle container creation.
///
/// Note: `youki create` spawns a container init process that remains running.
/// The parent youki process writes a PID file and then exec's into the init.
/// We spawn the process and poll for the PID file to detect completion.
pub async fn handle_create(
    container_id: String,
    bundle_path: String,
    event_tx: mpsc::Sender<Event>,
    responder: oneshot::Sender<Result<CreateResponse, ContainerError>>,
    youki_path: Arc<PathBuf>,
    youki_root: Option<Arc<PathBuf>>,
) {
    let id = container_id.clone();
    let pid_file = format!("{}/container.pid", bundle_path);

    // Build args: optionally prepend --root <path>
    let mut args: Vec<String> = Vec::new();
    if let Some(ref root) = youki_root {
        args.push("--root".to_string());
        args.push(root.to_string_lossy().to_string());
    }
    args.extend([
        "create".to_string(),
        "--bundle".to_string(),
        bundle_path.clone(),
        "--pid-file".to_string(),
        pid_file.clone(),
        id.clone(),
    ]);

    info!(
        "Worker: Spawning youki create: {} {}",
        youki_path.display(),
        args.join(" ")
    );

    // Spawn youki create - it becomes the container init process and doesn't exit
    let mut child = match Command::new(youki_path.as_ref())
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
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

    // Poll for PID file to appear (indicates create phase completed)
    // Timeout after 30 seconds
    let pid_file_path = std::path::Path::new(&pid_file);
    let mut attempts = 0;
    const MAX_ATTEMPTS: u32 = 300; // 30 seconds at 100ms intervals

    loop {
        // Check if process exited with error
        match child.try_wait() {
            Ok(Some(status)) if !status.success() => {
                // Process exited with error - read stderr
                let mut stderr_buf = Vec::new();
                if let Some(mut stderr) = child.stderr.take() {
                    use tokio::io::AsyncReadExt;
                    let _ = stderr.read_to_end(&mut stderr_buf).await;
                }
                let stderr = String::from_utf8_lossy(&stderr_buf);
                error!("Worker: youki create failed: stderr='{}'", stderr);
                let err = ContainerError::YoukiCommand(format!(
                    "youki create exited with {}: stderr='{}'",
                    status, stderr
                ));
                let _ = event_tx
                    .send(Event::ContainerCreateFailed {
                        id,
                        error: err.to_string(),
                    })
                    .await;
                let _ = responder.send(Err(err));
                return;
            }
            Ok(Some(_)) => {
                // Process exited successfully - PID file should exist
            }
            Ok(None) => {
                // Process still running - this is expected, it becomes the init
            }
            Err(e) => {
                let err =
                    ContainerError::YoukiCommand(format!("Failed to check youki status: {e}"));
                let _ = event_tx
                    .send(Event::ContainerCreateFailed {
                        id,
                        error: err.to_string(),
                    })
                    .await;
                let _ = responder.send(Err(err));
                return;
            }
        }

        // Check if PID file exists
        if pid_file_path.exists() {
            break;
        }

        attempts += 1;
        if attempts >= MAX_ATTEMPTS {
            let err =
                ContainerError::YoukiCommand("Timeout waiting for container PID file".to_string());
            let _ = event_tx
                .send(Event::ContainerCreateFailed {
                    id,
                    error: err.to_string(),
                })
                .await;
            let _ = responder.send(Err(err));
            // Kill the process
            let _ = child.kill().await;
            return;
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
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
        // Don't remove PID file - youki state command may need it
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
    youki_root: Option<Arc<PathBuf>>,
) {
    let id = container_id.clone();
    let root_ref = youki_root.as_deref();
    let result = run_youki_command(&youki_path, root_ref, &["start", &id]).await;

    match result {
        Ok(_) => {
            // Check container state after start for debugging
            if let Ok(state_output) =
                run_youki_command(&youki_path, root_ref, &["state", &id]).await
            {
                info!(
                    "Worker: Container {} state after start: {}",
                    id,
                    state_output.trim()
                );
            }

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
    youki_root: Option<Arc<PathBuf>>,
) {
    let signal_str = signal.to_string();
    let result = run_youki_command(
        &youki_path,
        youki_root.as_deref(),
        &["kill", &container_id, &signal_str],
    )
    .await
    .map(|_| ());
    let _ = responder.send(result);
}

/// Handle container deletion.
pub async fn handle_delete(
    container_id: String,
    event_tx: mpsc::Sender<Event>,
    responder: oneshot::Sender<Result<(), ContainerError>>,
    youki_path: Arc<PathBuf>,
    youki_root: Option<Arc<PathBuf>>,
) {
    let id = container_id.clone();
    let result = run_youki_command(
        &youki_path,
        youki_root.as_deref(),
        &["delete", "--force", &id],
    )
    .await;

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
