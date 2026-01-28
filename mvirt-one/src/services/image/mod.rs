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
    pub config: ImageConfig,
}

/// Parsed image configuration (Entrypoint, Cmd, Env, etc.)
#[derive(Debug, Clone, Default)]
pub struct ImageConfig {
    pub entrypoint: Vec<String>,
    pub cmd: Vec<String>,
    pub env: Vec<String>,
    pub working_dir: String,
}

/// Information about a cached image.
#[derive(Debug, Clone)]
pub struct ImageInfo {
    pub image_id: String,
    pub image_ref: String,
    pub rootfs_path: String,
    pub state: ImageState,
    pub config: ImageConfig,
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

/// Parse image config from OCI image config JSON blob.
pub fn parse_image_config(config_json: &[u8]) -> ImageConfig {
    use serde::Deserialize;

    #[derive(Deserialize, Default)]
    struct OciImageSpec {
        config: Option<OciImageConfigInner>,
    }

    #[derive(Deserialize, Default)]
    struct OciImageConfigInner {
        #[serde(rename = "Entrypoint")]
        entrypoint: Option<Vec<String>>,
        #[serde(rename = "Cmd")]
        cmd: Option<Vec<String>>,
        #[serde(rename = "Env")]
        env: Option<Vec<String>>,
        #[serde(rename = "WorkingDir")]
        working_dir: Option<String>,
    }

    match serde_json::from_slice::<OciImageSpec>(config_json) {
        Ok(spec) => {
            let config = spec.config.unwrap_or_default();
            ImageConfig {
                entrypoint: config.entrypoint.unwrap_or_default(),
                cmd: config.cmd.unwrap_or_default(),
                env: config.env.unwrap_or_default(),
                working_dir: config.working_dir.unwrap_or_default(),
            }
        }
        Err(e) => {
            log::warn!("Failed to parse image config: {}", e);
            ImageConfig::default()
        }
    }
}
