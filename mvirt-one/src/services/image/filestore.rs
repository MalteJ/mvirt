//! FileStore - Manages image storage on disk.
//!
//! Handles layer extraction and rootfs assembly.
//! Based on FeOS image-service/filestore.rs pattern.

use super::{ImageInfo, ImageState, PulledImageData};
use crate::error::ImageError;
use flate2::read::GzDecoder;
use log::{error, info, warn};
use oci_distribution::manifest;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use tar::Archive;
use tokio::fs;
use tokio::sync::{mpsc, oneshot};

/// Metadata stored alongside image files.
#[derive(Serialize, Deserialize)]
struct ImageMetadata {
    image_ref: String,
}

/// Commands for the FileStore actor.
pub enum FileCommand {
    Store {
        image_id: String,
        image_ref: String,
        image_data: PulledImageData,
        responder: oneshot::Sender<Result<String, ImageError>>,
    },
    Delete {
        image_id: String,
        responder: oneshot::Sender<Result<(), ImageError>>,
    },
    Scan {
        responder: oneshot::Sender<HashMap<String, ImageInfo>>,
    },
}

/// FileStore actor - manages image files on disk.
pub struct FileStore {
    command_rx: mpsc::Receiver<FileCommand>,
    command_tx: mpsc::Sender<FileCommand>,
    base_dir: PathBuf,
}

impl FileStore {
    /// Create a new FileStore.
    pub fn new(base_dir: PathBuf) -> Self {
        let (command_tx, command_rx) = mpsc::channel(32);
        Self {
            command_rx,
            command_tx,
            base_dir,
        }
    }

    /// Get a sender for sending commands to this FileStore.
    pub fn get_command_sender(&self) -> mpsc::Sender<FileCommand> {
        self.command_tx.clone()
    }

    /// Run the FileStore actor loop.
    pub async fn run(mut self) {
        info!("FileStore: Running and waiting for commands");
        while let Some(cmd) = self.command_rx.recv().await {
            self.handle_command(cmd).await;
        }
        info!("FileStore: Channel closed, shutting down");
    }

    async fn handle_command(&mut self, cmd: FileCommand) {
        match cmd {
            FileCommand::Store {
                image_id,
                image_ref,
                image_data,
                responder,
            } => {
                info!("FileStore: Storing image {}", image_id);
                let final_dir = self.base_dir.join(&image_id);
                let result = Self::store_image_impl(&final_dir, image_data, &image_ref).await;
                let response =
                    result.map(|_| final_dir.join("rootfs").to_string_lossy().to_string());
                let _ = responder.send(response);
            }
            FileCommand::Delete {
                image_id,
                responder,
            } => {
                info!("FileStore: Deleting image {}", image_id);
                let image_dir = self.base_dir.join(&image_id);
                let result = fs::remove_dir_all(&image_dir)
                    .await
                    .map_err(ImageError::Storage);
                let _ = responder.send(result);
            }
            FileCommand::Scan { responder } => {
                info!("FileStore: Scanning for existing images");
                let store = Self::scan_images_impl(&self.base_dir).await;
                let _ = responder.send(store);
            }
        }
    }

    async fn store_image_impl(
        final_dir: &Path,
        image_data: PulledImageData,
        image_ref: &str,
    ) -> Result<(), ImageError> {
        fs::create_dir_all(final_dir)
            .await
            .map_err(ImageError::Storage)?;

        let rootfs_path = final_dir.join("rootfs");
        fs::create_dir_all(&rootfs_path)
            .await
            .map_err(ImageError::Storage)?;

        for layer in image_data.layers {
            match layer.media_type.as_str() {
                manifest::IMAGE_LAYER_GZIP_MEDIA_TYPE
                | manifest::IMAGE_DOCKER_LAYER_GZIP_MEDIA_TYPE => {
                    let rootfs = rootfs_path.clone();
                    let layer_data = layer.data;

                    // Extract layer in blocking task
                    tokio::task::spawn_blocking(move || {
                        let cursor = Cursor::new(layer_data);
                        let decoder = GzDecoder::new(cursor);
                        let mut archive = Archive::new(decoder);
                        archive.unpack(&rootfs)
                    })
                    .await
                    .map_err(|e| ImageError::LayerExtraction(e.to_string()))?
                    .map_err(|e| ImageError::LayerExtraction(e.to_string()))?;
                }
                _ => {
                    warn!(
                        "FileStore: Skipping layer with unsupported media type: {}",
                        layer.media_type
                    );
                }
            }
        }

        // Write config.json
        fs::write(final_dir.join("config.json"), image_data.config)
            .await
            .map_err(ImageError::Storage)?;

        // Write metadata.json
        let metadata = ImageMetadata {
            image_ref: image_ref.to_string(),
        };
        let metadata_json = serde_json::to_string_pretty(&metadata)
            .map_err(|e| ImageError::Storage(std::io::Error::other(e)))?;
        fs::write(final_dir.join("metadata.json"), metadata_json)
            .await
            .map_err(ImageError::Storage)?;

        info!(
            "FileStore: Image stored successfully at {}",
            final_dir.display()
        );
        Ok(())
    }

    async fn scan_images_impl(base_dir: &Path) -> HashMap<String, ImageInfo> {
        let mut store = HashMap::new();

        let mut entries = match fs::read_dir(base_dir).await {
            Ok(entries) => entries,
            Err(e) => {
                error!(
                    "FileStore: Failed to read image directory {}: {}",
                    base_dir.display(),
                    e
                );
                return store;
            }
        };

        while let Some(entry) = entries.next_entry().await.ok().flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            if let Some(uuid) = path.file_name().and_then(|s| s.to_str()) {
                let metadata_path = path.join("metadata.json");
                let rootfs_path = path.join("rootfs");

                if metadata_path.exists() && rootfs_path.exists() {
                    if let Ok(content) = fs::read_to_string(&metadata_path).await {
                        if let Ok(metadata) = serde_json::from_str::<ImageMetadata>(&content) {
                            let image_info = ImageInfo {
                                image_id: uuid.to_string(),
                                image_ref: metadata.image_ref,
                                rootfs_path: rootfs_path.to_string_lossy().to_string(),
                                state: ImageState::Ready,
                            };
                            store.insert(uuid.to_string(), image_info);
                        } else {
                            warn!("FileStore: Could not parse metadata for {}", uuid);
                        }
                    } else {
                        warn!("FileStore: Could not read metadata for {}", uuid);
                    }
                }
            }
        }

        info!("FileStore: Scan complete. Found {} images.", store.len());
        store
    }
}
