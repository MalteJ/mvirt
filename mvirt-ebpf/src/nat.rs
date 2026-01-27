//! NAT configuration via nftables for external traffic.

use ipnet::{Ipv4Net, Ipv6Net};
use std::io;
use std::process::Command;
use thiserror::Error;
use tracing::{info, warn};

/// NAT configuration errors.
#[derive(Debug, Error)]
pub enum NatError {
    #[error("Failed to execute nft command: {0}")]
    Command(io::Error),

    #[error("nft command failed: {0}")]
    NftFailed(String),
}

pub type Result<T> = std::result::Result<T, NatError>;

const TABLE_NAME: &str = "mvirt_ebpf";
const NAT_CHAIN: &str = "postrouting";

/// Initialize the nftables table and chains.
pub fn init_nftables() -> Result<()> {
    // Create table if not exists
    let output = Command::new("nft")
        .args(["add", "table", "inet", TABLE_NAME])
        .output()
        .map_err(NatError::Command)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Ignore "already exists" error
        if !stderr.contains("exists") {
            return Err(NatError::NftFailed(stderr.to_string()));
        }
    }

    // Create NAT chain for postrouting
    let output = Command::new("nft")
        .args([
            "add",
            "chain",
            "inet",
            TABLE_NAME,
            NAT_CHAIN,
            "{ type nat hook postrouting priority srcnat; }",
        ])
        .output()
        .map_err(NatError::Command)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("exists") {
            return Err(NatError::NftFailed(stderr.to_string()));
        }
    }

    info!(table = TABLE_NAME, "nftables initialized");
    Ok(())
}

/// Add masquerade rule for a subnet going out through an interface.
pub fn add_masquerade_v4(subnet: Ipv4Net, out_iface: &str) -> Result<()> {
    let rule = format!("ip saddr {} oifname {} masquerade", subnet, out_iface);

    let output = Command::new("nft")
        .args(["add", "rule", "inet", TABLE_NAME, NAT_CHAIN, &rule])
        .output()
        .map_err(NatError::Command)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(NatError::NftFailed(stderr.to_string()));
    }

    info!(subnet = %subnet, out_iface, "IPv4 masquerade rule added");
    Ok(())
}

/// Add masquerade rule for IPv6 subnet.
pub fn add_masquerade_v6(prefix: Ipv6Net, out_iface: &str) -> Result<()> {
    let rule = format!("ip6 saddr {} oifname {} masquerade", prefix, out_iface);

    let output = Command::new("nft")
        .args(["add", "rule", "inet", TABLE_NAME, NAT_CHAIN, &rule])
        .output()
        .map_err(NatError::Command)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(NatError::NftFailed(stderr.to_string()));
    }

    info!(prefix = %prefix, out_iface, "IPv6 masquerade rule added");
    Ok(())
}

/// Remove masquerade rule for a subnet.
pub fn remove_masquerade_v4(subnet: Ipv4Net, out_iface: &str) -> Result<()> {
    // List rules with handles
    let output = Command::new("nft")
        .args(["-a", "list", "chain", "inet", TABLE_NAME, NAT_CHAIN])
        .output()
        .map_err(NatError::Command)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(NatError::NftFailed(stderr.to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let search_pattern = format!("ip saddr {} oifname", subnet);

    // Find and delete matching rules
    for line in stdout.lines() {
        if line.contains(&search_pattern) && line.contains(out_iface) {
            // Extract handle number
            if let Some(handle) = extract_handle(line) {
                let output = Command::new("nft")
                    .args([
                        "delete",
                        "rule",
                        "inet",
                        TABLE_NAME,
                        NAT_CHAIN,
                        "handle",
                        &handle.to_string(),
                    ])
                    .output()
                    .map_err(NatError::Command)?;

                if output.status.success() {
                    info!(subnet = %subnet, out_iface, "IPv4 masquerade rule removed");
                }
            }
        }
    }

    Ok(())
}

/// Remove masquerade rule for IPv6 prefix.
pub fn remove_masquerade_v6(prefix: Ipv6Net, out_iface: &str) -> Result<()> {
    let output = Command::new("nft")
        .args(["-a", "list", "chain", "inet", TABLE_NAME, NAT_CHAIN])
        .output()
        .map_err(NatError::Command)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(NatError::NftFailed(stderr.to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let search_pattern = format!("ip6 saddr {} oifname", prefix);

    for line in stdout.lines() {
        if line.contains(&search_pattern)
            && line.contains(out_iface)
            && let Some(handle) = extract_handle(line)
        {
            let output = Command::new("nft")
                .args([
                    "delete",
                    "rule",
                    "inet",
                    TABLE_NAME,
                    NAT_CHAIN,
                    "handle",
                    &handle.to_string(),
                ])
                .output()
                .map_err(NatError::Command)?;

            if output.status.success() {
                info!(prefix = %prefix, out_iface, "IPv6 masquerade rule removed");
            }
        }
    }

    Ok(())
}

/// Clean up all nftables rules on shutdown.
pub fn cleanup_nftables() -> Result<()> {
    let output = Command::new("nft")
        .args(["delete", "table", "inet", TABLE_NAME])
        .output()
        .map_err(NatError::Command)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Ignore "no such" error
        if !stderr.contains("No such") {
            warn!(error = %stderr, "Failed to cleanup nftables");
        }
    } else {
        info!(table = TABLE_NAME, "nftables cleaned up");
    }

    Ok(())
}

/// Extract handle number from nft output line.
/// Example: "  ip saddr 10.0.0.0/24 oifname eth0 masquerade # handle 5"
fn extract_handle(line: &str) -> Option<u32> {
    line.split("# handle ").nth(1)?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_handle() {
        let line = "  ip saddr 10.0.0.0/24 oifname eth0 masquerade # handle 5";
        assert_eq!(extract_handle(line), Some(5));

        let line = "  ip saddr 10.0.0.0/24 oifname eth0 masquerade # handle 123";
        assert_eq!(extract_handle(line), Some(123));

        let line = "  ip saddr 10.0.0.0/24 oifname eth0 masquerade";
        assert_eq!(extract_handle(line), None);
    }
}
