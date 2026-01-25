//! Test helpers for mvirt-one integration tests.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::timeout;

const YOUKI_VERSION: &str = "0.5.0";
const YOUKI_URL: &str =
    "https://github.com/youki-dev/youki/releases/download/v0.5.0/youki-0.5.0-x86_64-musl.tar.gz";

/// Test server wrapper that manages the mvirt-one process lifecycle.
pub struct TestServer {
    process: Child,
    pub addr: String,
}

impl TestServer {
    /// Start mvirt-one server as a subprocess.
    ///
    /// This will:
    /// 1. Check for root privileges (required for container operations)
    /// 2. Download youki if not already present
    /// 3. Clean up old test data
    /// 4. Start the mvirt-one server with the --youki parameter
    /// 5. Wait for the server to be ready
    pub async fn start() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // Root check
        if !nix::unistd::Uid::effective().is_root() {
            return Err(
                "Integration tests require root privileges. Run with: sudo cargo test".into(),
            );
        }

        // Ensure youki is installed
        let youki_path = ensure_youki_installed()?;

        // Clean up old test data
        let _ = std::fs::remove_dir_all("/tmp/mvirt-one");

        // Get path to the mvirt-one binary
        let bin = env!("CARGO_BIN_EXE_mvirt-one");

        // Start server with --youki parameter
        let process = Command::new(bin)
            .arg("--youki")
            .arg(&youki_path)
            .env("RUST_LOG", "info")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let addr = "http://127.0.0.1:50051".to_string();

        // Wait for server to be ready (up to 5 seconds)
        for _ in 0..50 {
            if TcpStream::connect("127.0.0.1:50051").await.is_ok() {
                return Ok(Self { process, addr });
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        Err("Server did not start in time".into())
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        // Send SIGTERM for graceful shutdown
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(self.process.id() as i32),
            nix::sys::signal::Signal::SIGTERM,
        );
        let _ = self.process.wait();

        // Clean up test data
        let _ = std::fs::remove_dir_all("/tmp/mvirt-one");
    }
}

/// Ensure youki is installed in the crate's target directory.
///
/// Downloads youki from GitHub if not already present.
fn ensure_youki_installed() -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    // Find the target directory
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target_dir = manifest_dir.join("target");
    let youki_path = target_dir.join("youki");

    if youki_path.exists() {
        return Ok(youki_path);
    }

    eprintln!(
        "youki not found, downloading v{} to {:?}...",
        YOUKI_VERSION, target_dir
    );

    // Create target directory
    std::fs::create_dir_all(&target_dir)?;

    // Download and extract
    let status = Command::new("sh")
        .args([
            "-c",
            &format!(
                "curl -sL {} | tar -xz -C {}",
                YOUKI_URL,
                target_dir.display()
            ),
        ])
        .status()?;

    if !status.success() {
        return Err("Failed to download youki".into());
    }

    eprintln!("youki installed to {:?}", youki_path);
    Ok(youki_path)
}

/// Check if a port is reachable.
pub async fn check_port(port: u16) -> bool {
    timeout(
        Duration::from_secs(5),
        TcpStream::connect(format!("127.0.0.1:{}", port)),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false)
}
