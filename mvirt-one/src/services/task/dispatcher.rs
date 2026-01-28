//! Task Service Dispatcher - Routes commands to workers.

use super::{Command, Event, worker};
use log::info;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Task Service Dispatcher.
/// Receives commands and dispatches them to worker functions.
pub struct TaskDispatcher {
    command_rx: mpsc::Receiver<Command>,
    event_tx: mpsc::Sender<Event>,
    /// Map of container_id -> pid for tracking running containers
    container_pids: HashMap<String, i32>,
    /// Path to youki binary
    youki_path: Arc<PathBuf>,
    /// Root directory for youki container state (None = use default /run/youki)
    youki_root: Option<Arc<PathBuf>>,
}

impl TaskDispatcher {
    /// Create a new Task Dispatcher.
    pub fn new(
        command_rx: mpsc::Receiver<Command>,
        event_tx: mpsc::Sender<Event>,
        youki_path: PathBuf,
        youki_root: Option<PathBuf>,
    ) -> Self {
        Self {
            command_rx,
            event_tx,
            container_pids: HashMap::new(),
            youki_path: Arc::new(youki_path),
            youki_root: youki_root.map(Arc::new),
        }
    }

    /// Run the dispatcher loop.
    pub async fn run(mut self) {
        info!("TaskDispatcher: Running and waiting for commands");

        while let Some(cmd) = self.command_rx.recv().await {
            self.handle_command(cmd).await;
        }

        info!("TaskDispatcher: Channel closed, shutting down");
    }

    async fn handle_command(&mut self, cmd: Command) {
        match cmd {
            Command::Create {
                container_id,
                bundle_path,
                responder,
            } => {
                info!("TaskDispatcher: Create container {}", container_id);
                worker::handle_create(
                    container_id,
                    bundle_path,
                    self.event_tx.clone(),
                    responder,
                    self.youki_path.clone(),
                    self.youki_root.clone(),
                )
                .await;
            }
            Command::Start {
                container_id,
                pid,
                responder,
            } => {
                info!("TaskDispatcher: Start container {}", container_id);
                self.container_pids.insert(container_id.clone(), pid);
                worker::handle_start(
                    container_id,
                    pid,
                    self.event_tx.clone(),
                    responder,
                    self.youki_path.clone(),
                    self.youki_root.clone(),
                )
                .await;
            }
            Command::Kill {
                container_id,
                signal,
                responder,
            } => {
                info!(
                    "TaskDispatcher: Kill container {} with signal {}",
                    container_id, signal
                );
                worker::handle_kill(
                    container_id,
                    signal,
                    responder,
                    self.youki_path.clone(),
                    self.youki_root.clone(),
                )
                .await;
            }
            Command::Delete {
                container_id,
                responder,
            } => {
                info!("TaskDispatcher: Delete container {}", container_id);
                self.container_pids.remove(&container_id);
                worker::handle_delete(
                    container_id,
                    self.event_tx.clone(),
                    responder,
                    self.youki_path.clone(),
                    self.youki_root.clone(),
                )
                .await;
            }
        }
    }
}
