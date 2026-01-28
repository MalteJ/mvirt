//! mvirt-one - MicroVM Init System for isolated Pods.
//!
//! A minimal init system that runs as PID 1 inside MicroVMs, managing
//! containers via youki and communicating with the host via vsock.
//!
//! ## Architecture
//!
//! - **Pod Service**: High-level pod lifecycle management
//! - **Task Service**: Low-level OCI runtime (youki) interface
//! - **Image Service**: OCI image pulling and layer extraction
//!
//! ## Dual-Mode Operation
//!
//! - **PID 1 mode**: Full init responsibilities (mounting, network, vsock)
//! - **Local mode**: For development/testing without a VM

pub mod error;
pub mod proto;
pub mod services;
pub mod utils;

use crate::services::image::{self, Command as ImageCommand};
use crate::services::pod::{Command as PodCommand, PodApiHandler, PodDispatcher};
use crate::services::task::{Command as TaskCommand, Event as TaskEvent, TaskDispatcher};
use log::info;
use std::path::PathBuf;
use tokio::sync::mpsc;

/// Configuration for one services.
pub struct Config {
    /// Base directory for images.
    pub images_dir: PathBuf,
    /// Base directory for pod bundles.
    pub pods_dir: PathBuf,
    /// Path to youki binary.
    pub youki_path: PathBuf,
    /// Root directory for youki container state (default: /run/youki).
    pub youki_root: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        if std::process::id() == 1 {
            // Running as PID 1 in VM
            Self {
                images_dir: PathBuf::from("/run/images"),
                pods_dir: PathBuf::from("/run/pods"),
                youki_path: PathBuf::from("/usr/bin/youki"),
                youki_root: None, // Use youki's default (/run/youki)
            }
        } else {
            // Running locally for development
            Self {
                images_dir: PathBuf::from("/tmp/mvirt-one/images"),
                pods_dir: PathBuf::from("/tmp/mvirt-one/pods"),
                youki_path: PathBuf::from("youki"),
                youki_root: None, // Use youki's default (/run/youki)
            }
        }
    }
}

/// Service handles for communicating with one services.
pub struct Services {
    pub image_tx: mpsc::Sender<ImageCommand>,
    pub task_tx: mpsc::Sender<TaskCommand>,
    pub pod_tx: mpsc::Sender<PodCommand>,
    pub shutdown_tx: mpsc::Sender<()>,
    pub shutdown_rx: mpsc::Receiver<()>,
}

/// Initialize all one services.
pub async fn initialize_services(config: Config) -> anyhow::Result<Services> {
    info!("Initializing one services");

    // Create directories
    tokio::fs::create_dir_all(&config.images_dir).await?;
    tokio::fs::create_dir_all(&config.pods_dir).await?;

    // Initialize Image Service
    let image_tx = image::orchestrator::initialize_image_service(config.images_dir.clone()).await;
    info!("Image Service initialized");

    // Initialize Task Service
    let (task_tx, task_rx) = mpsc::channel::<TaskCommand>(32);
    let (task_event_tx, task_event_rx) = mpsc::channel::<TaskEvent>(32);
    let task_dispatcher = TaskDispatcher::new(
        task_rx,
        task_event_tx,
        config.youki_path.clone(),
        config.youki_root,
    );
    tokio::spawn(async move {
        task_dispatcher.run().await;
    });
    info!("Task Service initialized");

    // Initialize Pod Service
    let (pod_tx, pod_rx) = mpsc::channel::<PodCommand>(32);
    let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>(1);
    let pod_dispatcher = PodDispatcher::new(
        pod_rx,
        task_event_rx,
        image_tx.clone(),
        task_tx.clone(),
        config.pods_dir,
    );
    tokio::spawn(async move {
        pod_dispatcher.run().await;
    });
    info!("Pod Service initialized");

    Ok(Services {
        image_tx,
        task_tx,
        pod_tx,
        shutdown_tx,
        shutdown_rx,
    })
}

/// Create a gRPC service for the Pod API.
pub fn create_api_handler(
    pod_tx: mpsc::Sender<PodCommand>,
    shutdown_tx: mpsc::Sender<()>,
) -> PodApiHandler {
    PodApiHandler::new(pod_tx, shutdown_tx)
}
