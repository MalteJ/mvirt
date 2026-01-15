//! System information collection for the System view.
//!
//! Collects detailed host information from various sources:
//! - sysinfo crate for CPU/memory basics
//! - /proc/cpuinfo for CPU model and flags
//! - /sys/devices/system/node for NUMA topology
//! - smartctl for disk health
//! - /sys/class/net for NIC details

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::proto::{CpuCore, CpuInfo, DiskInfo, HostInfo, MemoryInfo, NicInfo, NumaNode};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, Networks, RefreshKind, System};

/// Collect host information (hostname, kernel, uptime)
pub fn collect_host_info() -> HostInfo {
    let hostname = fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let kernel_version = fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let uptime_seconds = fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(String::from))
        .and_then(|s| s.parse::<f64>().ok())
        .map(|f| f as u64)
        .unwrap_or(0);

    HostInfo {
        hostname,
        kernel_version,
        uptime_seconds,
    }
}

/// Collect detailed CPU information
pub fn collect_cpu_info(sys: &System) -> CpuInfo {
    // Parse /proc/cpuinfo for model, vendor, and flags
    let cpuinfo = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let mut model = String::new();
    let mut vendor = String::new();
    let mut flags: Vec<String> = Vec::new();
    let mut physical_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut core_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    for line in cpuinfo.lines() {
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim();
            match key {
                "model name" if model.is_empty() => model = value.to_string(),
                "vendor_id" if vendor.is_empty() => vendor = value.to_string(),
                "flags" if flags.is_empty() => {
                    // Only keep interesting flags
                    let interesting = [
                        "vmx",
                        "svm",
                        "ept",
                        "npt",
                        "avx",
                        "avx2",
                        "avx512f",
                        "aes",
                        "sse4_1",
                        "sse4_2",
                        "popcnt",
                        "rdrand",
                        "hypervisor",
                    ];
                    flags = value
                        .split_whitespace()
                        .filter(|f| interesting.contains(f))
                        .map(String::from)
                        .collect();
                }
                "physical id" => {
                    physical_ids.insert(value.to_string());
                }
                "core id" => {
                    core_ids.insert(value.to_string());
                }
                _ => {}
            }
        }
    }

    let logical_cores = sys.cpus().len() as u32;
    let sockets = physical_ids.len().max(1) as u32;
    // Physical cores: unique core IDs per socket, or estimate from logical/2 if HT
    let physical_cores = if core_ids.is_empty() {
        // No core id info, estimate
        (logical_cores / sockets / 2).max(1) * sockets
    } else {
        (core_ids.len() as u32 * sockets).max(1)
    };

    // Collect per-core info
    let cores = collect_cpu_cores(sys);

    CpuInfo {
        model,
        vendor,
        physical_cores,
        logical_cores,
        sockets,
        cores,
        flags,
    }
}

/// Collect per-core CPU information
fn collect_cpu_cores(sys: &System) -> Vec<CpuCore> {
    let numa_map = build_cpu_numa_map();

    sys.cpus()
        .iter()
        .enumerate()
        .map(|(id, cpu)| {
            // Try to get current frequency from sysfs (more accurate)
            let freq_mhz = read_cpu_frequency(id as u32).unwrap_or_else(|| cpu.frequency());

            CpuCore {
                id: id as u32,
                frequency_mhz: freq_mhz,
                usage_percent: cpu.cpu_usage(),
                numa_node: numa_map.get(&(id as u32)).copied().unwrap_or(0),
            }
        })
        .collect()
}

/// Read current CPU frequency from sysfs
fn read_cpu_frequency(cpu_id: u32) -> Option<u64> {
    let path = format!(
        "/sys/devices/system/cpu/cpu{}/cpufreq/scaling_cur_freq",
        cpu_id
    );
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|khz| khz / 1000) // Convert kHz to MHz
}

/// Build a map of CPU ID -> NUMA node
fn build_cpu_numa_map() -> HashMap<u32, u32> {
    let mut map = HashMap::new();
    let numa_path = Path::new("/sys/devices/system/node");

    if !numa_path.exists() {
        return map;
    }

    if let Ok(entries) = fs::read_dir(numa_path) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("node")
                && let Ok(node_id) = name.trim_start_matches("node").parse::<u32>()
            {
                let cpulist_path = entry.path().join("cpulist");
                if let Ok(cpulist) = fs::read_to_string(&cpulist_path) {
                    for cpu_id in parse_cpu_list(&cpulist) {
                        map.insert(cpu_id, node_id);
                    }
                }
            }
        }
    }

    map
}

/// Parse a CPU list string like "0-3,8-11" into individual CPU IDs
fn parse_cpu_list(list: &str) -> Vec<u32> {
    let mut result = Vec::new();
    for part in list.trim().split(',') {
        if let Some((start, end)) = part.split_once('-') {
            if let (Ok(s), Ok(e)) = (start.parse::<u32>(), end.parse::<u32>()) {
                result.extend(s..=e);
            }
        } else if let Ok(id) = part.parse::<u32>() {
            result.push(id);
        }
    }
    result
}

/// Collect memory information
pub fn collect_memory_info(sys: &System) -> MemoryInfo {
    // Get hugepage info from /proc/meminfo
    let meminfo = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut hugepages_total = 0u64;
    let mut hugepages_free = 0u64;
    let mut hugepage_size_kb = 2048u64; // Default 2MB
    let mut cached_kb = 0u64;

    for line in meminfo.lines() {
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim().trim_end_matches(" kB").replace(' ', "");
            match key {
                "HugePages_Total" => hugepages_total = value.parse().unwrap_or(0),
                "HugePages_Free" => hugepages_free = value.parse().unwrap_or(0),
                "Hugepagesize" => hugepage_size_kb = value.parse().unwrap_or(2048),
                "Cached" => cached_kb = value.parse().unwrap_or(0),
                _ => {}
            }
        }
    }

    MemoryInfo {
        total_bytes: sys.total_memory(),
        available_bytes: sys.available_memory(),
        cached_bytes: cached_kb * 1024,
        swap_total_bytes: sys.total_swap(),
        swap_used_bytes: sys.used_swap(),
        hugepages_total,
        hugepages_free,
        hugepage_size_kb,
    }
}

/// Collect NUMA node information
pub fn collect_numa_nodes() -> Vec<NumaNode> {
    let numa_path = Path::new("/sys/devices/system/node");
    if !numa_path.exists() {
        return Vec::new();
    }

    let mut nodes = Vec::new();

    if let Ok(entries) = fs::read_dir(numa_path) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("node")
                && let Ok(node_id) = name.trim_start_matches("node").parse::<u32>()
            {
                let node_path = entry.path();

                // Read CPU list
                let cpu_ids = fs::read_to_string(node_path.join("cpulist"))
                    .map(|s| parse_cpu_list(&s))
                    .unwrap_or_default();

                // Read memory info from meminfo
                let (total_bytes, free_bytes) = parse_numa_meminfo(&node_path.join("meminfo"));

                nodes.push(NumaNode {
                    id: node_id,
                    total_memory_bytes: total_bytes,
                    free_memory_bytes: free_bytes,
                    cpu_ids,
                });
            }
        }
    }

    nodes.sort_by_key(|n| n.id);
    nodes
}

/// Parse NUMA node meminfo file
fn parse_numa_meminfo(path: &Path) -> (u64, u64) {
    let content = fs::read_to_string(path).unwrap_or_default();
    let mut total_kb = 0u64;
    let mut free_kb = 0u64;

    for line in content.lines() {
        // Format: "Node 0 MemTotal:       32768000 kB"
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            let key = parts[2].trim_end_matches(':');
            let value: u64 = parts[3].parse().unwrap_or(0);
            match key {
                "MemTotal" => total_kb = value,
                "MemFree" => free_kb = value,
                _ => {}
            }
        }
    }

    (total_kb * 1024, free_kb * 1024)
}

/// Collect disk information with SMART data
pub fn collect_disk_info() -> Vec<DiskInfo> {
    let mut disks = Vec::new();

    // List block devices from /sys/block
    let block_path = Path::new("/sys/block");
    if let Ok(entries) = fs::read_dir(block_path) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();

            // Skip virtual devices (loop, dm, ram, etc.)
            if name.starts_with("loop")
                || name.starts_with("dm-")
                || name.starts_with("ram")
                || name.starts_with("zram")
                || name.starts_with("nbd")
            {
                continue;
            }

            // Only include physical disks (sd*, nvme*, vd*, hd*)
            if !name.starts_with("sd")
                && !name.starts_with("nvme")
                && !name.starts_with("vd")
                && !name.starts_with("hd")
            {
                continue;
            }

            let device = format!("/dev/{}", name);
            let disk_path = entry.path();

            // Get size from sysfs
            let size_bytes = fs::read_to_string(disk_path.join("size"))
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok())
                .map(|sectors| sectors * 512)
                .unwrap_or(0);

            // Get model from sysfs
            let model = fs::read_to_string(disk_path.join("device/model"))
                .map(|s| s.trim().to_string())
                .unwrap_or_default();

            // Try to get SMART data
            let smart_data = get_smart_data(&device);

            disks.push(DiskInfo {
                device,
                model: smart_data.model.unwrap_or(model),
                serial: smart_data.serial.unwrap_or_default(),
                size_bytes,
                smart_available: smart_data.available,
                smart_healthy: smart_data.healthy,
                temperature_celsius: smart_data.temperature,
                power_on_hours: smart_data.power_on_hours,
            });
        }
    }

    disks.sort_by(|a, b| a.device.cmp(&b.device));
    disks
}

/// SMART data collected from smartctl
struct SmartData {
    available: bool,
    healthy: bool,
    model: Option<String>,
    serial: Option<String>,
    temperature: Option<i32>,
    power_on_hours: Option<u64>,
}

/// Get SMART data for a device using smartctl
fn get_smart_data(device: &str) -> SmartData {
    let mut data = SmartData {
        available: false,
        healthy: true,
        model: None,
        serial: None,
        temperature: None,
        power_on_hours: None,
    };

    // Run smartctl with JSON output
    let output = match Command::new("smartctl").args(["-j", "-a", device]).output() {
        Ok(o) => o,
        Err(_) => return data,
    };

    if !output.status.success() && output.stdout.is_empty() {
        return data;
    }

    // Parse JSON output
    let json: serde_json::Value = match serde_json::from_slice(&output.stdout) {
        Ok(v) => v,
        Err(_) => return data,
    };

    data.available = json
        .get("smart_status")
        .and_then(|s| s.get("passed"))
        .is_some();

    data.healthy = json
        .get("smart_status")
        .and_then(|s| s.get("passed"))
        .and_then(|p| p.as_bool())
        .unwrap_or(true);

    data.model = json
        .get("model_name")
        .and_then(|v| v.as_str())
        .map(String::from);

    data.serial = json
        .get("serial_number")
        .and_then(|v| v.as_str())
        .map(String::from);

    // Temperature from multiple possible locations
    data.temperature = json
        .get("temperature")
        .and_then(|t| t.get("current"))
        .and_then(|v| v.as_i64())
        .map(|t| t as i32)
        .or_else(|| {
            // Try ata_smart_attributes
            json.get("ata_smart_attributes")
                .and_then(|a| a.get("table"))
                .and_then(|t| t.as_array())
                .and_then(|arr| {
                    arr.iter()
                        .find(|attr| {
                            attr.get("id").and_then(|id| id.as_i64()) == Some(194)
                                || attr.get("name").and_then(|n| n.as_str())
                                    == Some("Temperature_Celsius")
                        })
                        .and_then(|attr| attr.get("raw").and_then(|r| r.get("value")))
                        .and_then(|v| v.as_i64())
                        .map(|t| t as i32)
                })
        });

    // Power on hours
    data.power_on_hours = json
        .get("power_on_time")
        .and_then(|t| t.get("hours"))
        .and_then(|v| v.as_u64())
        .or_else(|| {
            json.get("ata_smart_attributes")
                .and_then(|a| a.get("table"))
                .and_then(|t| t.as_array())
                .and_then(|arr| {
                    arr.iter()
                        .find(|attr| attr.get("id").and_then(|id| id.as_i64()) == Some(9))
                        .and_then(|attr| attr.get("raw").and_then(|r| r.get("value")))
                        .and_then(|v| v.as_u64())
                })
        });

    data
}

/// Collect network interface information
pub fn collect_nic_info() -> Vec<NicInfo> {
    let mut nics = Vec::new();
    let networks = Networks::new_with_refreshed_list();

    for (name, data) in networks.iter() {
        // Skip loopback
        if name == "lo" {
            continue;
        }

        let net_path = Path::new("/sys/class/net").join(name);

        // Check if interface is up
        let is_up = fs::read_to_string(net_path.join("operstate"))
            .map(|s| s.trim() == "up")
            .unwrap_or(false);

        // Get MAC address
        let mac = fs::read_to_string(net_path.join("address"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        // Get speed (only meaningful for physical NICs)
        let speed_mbps = fs::read_to_string(net_path.join("speed"))
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
            .filter(|&s| s > 0)
            .map(|s| s as u32);

        // Get duplex
        let duplex = fs::read_to_string(net_path.join("duplex"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        // Get driver name
        let driver = fs::read_link(net_path.join("device/driver"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_default();

        // Get IP addresses from ip command (sysinfo doesn't provide this)
        let (ipv4, ipv6) = get_ip_addresses(name);

        nics.push(NicInfo {
            name: name.to_string(),
            mac,
            is_up,
            speed_mbps,
            duplex,
            ipv4,
            ipv6,
            driver,
            rx_bytes: data.received(),
            tx_bytes: data.transmitted(),
        });
    }

    nics.sort_by(|a, b| a.name.cmp(&b.name));
    nics
}

/// Get IP addresses for an interface
fn get_ip_addresses(interface: &str) -> (Vec<String>, Vec<String>) {
    let mut ipv4 = Vec::new();
    let mut ipv6 = Vec::new();

    // Use ip command to get addresses
    if let Ok(output) = Command::new("ip")
        .args(["-o", "addr", "show", interface])
        .output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                let family = parts[2];
                let addr = parts[3].split('/').next().unwrap_or("");
                match family {
                    "inet" => ipv4.push(addr.to_string()),
                    "inet6" => {
                        // Skip link-local addresses (fe80::)
                        if !addr.starts_with("fe80:") {
                            ipv6.push(addr.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    (ipv4, ipv6)
}

/// Create a new System instance with appropriate refresh settings
pub fn create_system() -> System {
    System::new_with_specifics(
        RefreshKind::new()
            .with_cpu(CpuRefreshKind::everything())
            .with_memory(MemoryRefreshKind::everything()),
    )
}

/// Refresh the system for a new reading
pub fn refresh_system(sys: &mut System) {
    sys.refresh_cpu_all();
    sys.refresh_memory();
}
