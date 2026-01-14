mod error;
mod log;
mod mount;
mod network;
mod service;
mod signals;

use service::ServiceManager;
use std::time::Duration;

fn main() -> ! {
    println!("[pideisn] Starting init process (PID 1)");

    // Phase 1: Mount filesystems
    mount::mount_all();

    // Phase 2: Setup signal handling
    signals::setup_signal_handlers();

    // Phase 3: Run async initialization
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to create tokio runtime");

    rt.block_on(async {
        // Configure network
        network::configure_all().await;

        // Start services
        let service_manager = ServiceManager::new();

        for config in service::default_services() {
            service_manager.register(config).await;
        }

        service_manager.start_all().await;

        // Main loop
        loop {
            // Reap zombie children
            signals::reap_children();

            // Check and restart services if needed
            service_manager.check_and_restart().await;

            // Sleep to avoid busy loop
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
}
