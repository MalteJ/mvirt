//! Image Service - OCI image pulling and storage.
//!
//! Downloads container images from OCI registries and extracts layers to rootfs.
//! Based on FeOS image-service pattern.

mod filestore;
pub mod orchestrator;
mod puller;

pub use filestore::FileStore;
pub use orchestrator::ImageOrchestrator;

use crate::error::ImageError;
use tokio::sync::oneshot;

/// Commands that can be sent to the Image Service.
#[derive(Debug)]
pub enum Command {
    /// Pull an image from a registry.
    Pull {
        image_ref: String,
        responder: oneshot::Sender<Result<PullResponse, ImageError>>,
    },
    /// Get information about a cached image.
    Get {
        image_id: String,
        responder: oneshot::Sender<Result<Option<ImageInfo>, ImageError>>,
    },
    /// List all cached images.
    List {
        responder: oneshot::Sender<Result<Vec<ImageInfo>, ImageError>>,
    },
    /// Delete a cached image.
    Delete {
        image_id: String,
        responder: oneshot::Sender<Result<(), ImageError>>,
    },
}

/// Response from image pull.
#[derive(Debug, Clone)]
pub struct PullResponse {
    pub image_id: String,
    pub rootfs_path: String,
}

/// Information about a cached image.
#[derive(Debug, Clone)]
pub struct ImageInfo {
    pub image_id: String,
    pub image_ref: String,
    pub rootfs_path: String,
    pub state: ImageState,
}

/// State of an image in the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageState {
    Downloading,
    Extracting,
    Ready,
    Failed,
}

/// Data from a pulled image.
#[derive(Debug)]
pub struct PulledImageData {
    pub config: Vec<u8>,
    pub layers: Vec<PulledLayer>,
}

/// A single layer from a pulled image.
#[derive(Debug)]
pub struct PulledLayer {
    pub media_type: String,
    pub data: Vec<u8>,
}
