//! Image Orchestrator - Coordinates image pulling and storage.
//!
//! Based on FeOS image-service/worker.rs pattern.

use super::filestore::{FileCommand, FileStore};
use super::puller::pull_oci_image;
use super::{Command, ImageInfo, ImageState, PullResponse};
use crate::error::ImageError;
use log::{error, info};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

/// Image Orchestrator - coordinates pulling and storage of images.
pub struct ImageOrchestrator {
    command_rx: mpsc::Receiver<Command>,
    command_tx: mpsc::Sender<Command>,
    filestore_tx: mpsc::Sender<FileCommand>,
    store: HashMap<String, ImageInfo>,
}

impl ImageOrchestrator {
    /// Create a new Image Orchestrator.
    pub fn new(filestore_tx: mpsc::Sender<FileCommand>) -> Self {
        let (command_tx, command_rx) = mpsc::channel(32);
        Self {
            command_rx,
            command_tx,
            filestore_tx,
            store: HashMap::new(),
        }
    }

    /// Get a sender for sending commands to this orchestrator.
    pub fn get_command_sender(&self) -> mpsc::Sender<Command> {
        self.command_tx.clone()
    }

    /// Run the orchestrator loop.
    pub async fn run(mut self) {
        // Scan for existing images on startup
        let (responder, resp_rx) = oneshot::channel();
        if self
            .filestore_tx
            .send(FileCommand::Scan { responder })
            .await
            .is_ok()
            && let Ok(initial_store) = resp_rx.await
        {
            self.store = initial_store;
        }

        info!("ImageOrchestrator: Running and waiting for commands");
        while let Some(cmd) = self.command_rx.recv().await {
            self.handle_command(cmd).await;
        }
        info!("ImageOrchestrator: Channel closed, shutting down");
    }

    async fn handle_command(&mut self, cmd: Command) {
        match cmd {
            Command::Pull {
                image_ref,
                responder,
            } => {
                // Check if we already have this image
                for (id, info) in &self.store {
                    if info.image_ref == image_ref && info.state == ImageState::Ready {
                        info!(
                            "ImageOrchestrator: Image {} already cached as {}",
                            image_ref, id
                        );
                        let _ = responder.send(Ok(PullResponse {
                            image_id: id.clone(),
                            rootfs_path: info.rootfs_path.clone(),
                        }));
                        return;
                    }
                }

                let image_id = Uuid::new_v4().to_string();
                info!(
                    "ImageOrchestrator: Starting pull for '{}', assigned ID {}",
                    image_ref, image_id
                );

                // Mark as downloading
                self.store.insert(
                    image_id.clone(),
                    ImageInfo {
                        image_id: image_id.clone(),
                        image_ref: image_ref.clone(),
                        rootfs_path: String::new(),
                        state: ImageState::Downloading,
                    },
                );

                // Pull the image
                let image_data = match pull_oci_image(&image_ref).await {
                    Ok(data) => data,
                    Err(e) => {
                        error!("ImageOrchestrator: Pull failed for {}: {}", image_ref, e);
                        if let Some(info) = self.store.get_mut(&image_id) {
                            info.state = ImageState::Failed;
                        }
                        let _ = responder.send(Err(e));
                        return;
                    }
                };

                // Update state to extracting
                if let Some(info) = self.store.get_mut(&image_id) {
                    info.state = ImageState::Extracting;
                }

                // Store the image
                let (store_responder, store_rx) = oneshot::channel();
                let store_cmd = FileCommand::Store {
                    image_id: image_id.clone(),
                    image_ref: image_ref.clone(),
                    image_data,
                    responder: store_responder,
                };

                if self.filestore_tx.send(store_cmd).await.is_err() {
                    error!("ImageOrchestrator: Failed to send to FileStore");
                    if let Some(info) = self.store.get_mut(&image_id) {
                        info.state = ImageState::Failed;
                    }
                    let _ = responder.send(Err(ImageError::Storage(std::io::Error::other(
                        "FileStore channel closed",
                    ))));
                    return;
                }

                match store_rx.await {
                    Ok(Ok(rootfs_path)) => {
                        info!("ImageOrchestrator: Image {} stored successfully", image_id);
                        if let Some(info) = self.store.get_mut(&image_id) {
                            info.state = ImageState::Ready;
                            info.rootfs_path = rootfs_path.clone();
                        }
                        let _ = responder.send(Ok(PullResponse {
                            image_id,
                            rootfs_path,
                        }));
                    }
                    Ok(Err(e)) => {
                        error!("ImageOrchestrator: FileStore failed to store image: {}", e);
                        if let Some(info) = self.store.get_mut(&image_id) {
                            info.state = ImageState::Failed;
                        }
                        let _ = responder.send(Err(e));
                    }
                    Err(_) => {
                        error!("ImageOrchestrator: FileStore dropped response channel");
                        if let Some(info) = self.store.get_mut(&image_id) {
                            info.state = ImageState::Failed;
                        }
                        let _ = responder.send(Err(ImageError::Storage(std::io::Error::other(
                            "FileStore response channel dropped",
                        ))));
                    }
                }
            }
            Command::Get {
                image_id,
                responder,
            } => {
                let result = self.store.get(&image_id).cloned();
                let _ = responder.send(Ok(result));
            }
            Command::List { responder } => {
                let images: Vec<ImageInfo> = self.store.values().cloned().collect();
                let _ = responder.send(Ok(images));
            }
            Command::Delete {
                image_id,
                responder,
            } => {
                info!("ImageOrchestrator: Deleting image {}", image_id);
                self.store.remove(&image_id);

                let (file_resp_tx, file_resp_rx) = oneshot::channel();
                let file_cmd = FileCommand::Delete {
                    image_id: image_id.clone(),
                    responder: file_resp_tx,
                };

                if self.filestore_tx.send(file_cmd).await.is_err() {
                    error!("ImageOrchestrator: Failed to send delete to FileStore");
                }

                // Wait for deletion but don't fail if it errors
                let _ = file_resp_rx.await;
                let _ = responder.send(Ok(()));
            }
        }
    }
}

/// Initialize the Image Service.
pub async fn initialize_image_service(base_dir: PathBuf) -> mpsc::Sender<Command> {
    let filestore = FileStore::new(base_dir);
    let filestore_tx = filestore.get_command_sender();
    tokio::spawn(async move {
        filestore.run().await;
    });
    info!("ImageService: FileStore started");

    let orchestrator = ImageOrchestrator::new(filestore_tx);
    let orchestrator_tx = orchestrator.get_command_sender();
    tokio::spawn(async move {
        orchestrator.run().await;
    });
    info!("ImageService: Orchestrator started");

    orchestrator_tx
}
