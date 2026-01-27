use clap::{Parser, Subcommand};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use tabled::{Table, Tabled};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_stream::StreamExt;
use tonic::transport::Channel;

pub mod proto {
    tonic::include_proto!("mvirt");
}

pub mod zfs_proto {
    tonic::include_proto!("mvirt.zfs");
}

pub mod net_proto {
    tonic::include_proto!("mvirt.net");
}

mod tui;

use mvirt_log::LogServiceClient;
use net_proto::net_service_client::NetServiceClient;
use proto::pod_service_client::PodServiceClient;
use proto::vm_service_client::VmServiceClient;
use proto::*;
use zfs_proto::zfs_service_client::ZfsServiceClient;

#[derive(Parser)]
#[command(name = "mvirt")]
#[command(about = "CLI for mvirt VM manager", long_about = None)]
struct Cli {
    /// gRPC server address for mvirt-vmm
    #[arg(short, long, default_value = "http://[::1]:50051")]
    server: String,

    /// gRPC server address for mvirt-zfs (storage)
    #[arg(long, default_value = "http://[::1]:50053")]
    zfs_server: String,

    /// gRPC server address for mvirt-log (logging)
    #[arg(long, default_value = "http://[::1]:50052")]
    log_server: String,

    /// gRPC server address for mvirt-net (networking)
    #[arg(long, default_value = "http://[::1]:50054")]
    net_server: String,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new VM
    Create {
        /// VM name (optional)
        #[arg(short, long)]
        name: Option<String>,

        /// Number of vCPUs
        #[arg(long, default_value = "1")]
        vcpus: u32,

        /// Memory in MB
        #[arg(long, default_value = "512")]
        memory: u64,

        /// Boot mode: disk (default) or kernel
        #[arg(long, default_value = "disk")]
        boot: String,

        /// Path to kernel (required for kernel boot)
        #[arg(long)]
        kernel: Option<String>,

        /// Path to initramfs (for kernel boot)
        #[arg(long)]
        initramfs: Option<String>,

        /// Kernel command line (for kernel boot)
        #[arg(long)]
        cmdline: Option<String>,

        /// Path to disk image (required for disk boot)
        #[arg(long)]
        disk: Option<String>,

        /// Path to cloud-init user-data file
        #[arg(long)]
        user_data: Option<std::path::PathBuf>,

        /// Enable nested virtualization
        #[arg(long)]
        nested_virt: bool,
    },

    /// List all VMs
    List,

    /// Get VM details
    Get {
        /// VM ID
        id: String,
    },

    /// Delete a VM
    Delete {
        /// VM ID
        id: String,
    },

    /// Start a VM
    Start {
        /// VM ID
        id: String,
    },

    /// Stop a VM (graceful shutdown)
    Stop {
        /// VM ID
        id: String,

        /// Timeout in seconds (0 = wait indefinitely)
        #[arg(short, long, default_value = "30")]
        timeout: u32,
    },

    /// Kill a VM (force stop)
    Kill {
        /// VM ID
        id: String,
    },

    /// Connect to VM console (exit with Ctrl+a t)
    Console {
        /// VM ID
        id: String,
    },

    /// Import a template from URL or file
    Import {
        /// Template name
        name: String,

        /// Source URL or file path
        source: String,
    },

    /// Storage pool operations
    Pool,

    /// Volume operations
    #[command(subcommand)]
    Volume(VolumeCommands),

    /// Snapshot operations
    #[command(subcommand)]
    Snapshot(SnapshotCommands),

    /// Template operations
    #[command(subcommand)]
    Template(TemplateCommands),

    /// Network operations
    #[command(subcommand)]
    Network(NetworkCommands),

    /// NIC operations
    #[command(subcommand)]
    Nic(NicCommands),

    /// Pod operations (container pods in MicroVMs)
    #[command(subcommand)]
    Pod(PodCommands),
}

#[derive(Subcommand)]
enum VolumeCommands {
    /// List all volumes
    List,

    /// Create an empty volume
    Create {
        /// Volume name
        name: String,

        /// Size in GB
        #[arg(short, long)]
        size: u64,
    },

    /// Delete a volume
    Delete {
        /// Volume name
        name: String,
    },

    /// Resize a volume
    Resize {
        /// Volume name
        name: String,

        /// New size in GB
        #[arg(short, long)]
        size: u64,
    },
}

#[derive(Subcommand)]
enum SnapshotCommands {
    /// List snapshots of a volume
    List {
        /// Volume name
        volume: String,
    },

    /// Create a snapshot
    Create {
        /// Volume name
        volume: String,

        /// Snapshot name
        name: String,
    },

    /// Delete a snapshot
    Delete {
        /// Volume name
        volume: String,

        /// Snapshot name
        name: String,
    },

    /// Rollback volume to snapshot
    Rollback {
        /// Volume name
        volume: String,

        /// Snapshot name
        name: String,
    },

    /// Promote snapshot to a new template
    Promote {
        /// Volume name
        volume: String,

        /// Snapshot name
        snapshot: String,

        /// New template name
        template: String,
    },
}

#[derive(Subcommand)]
enum TemplateCommands {
    /// List all templates
    List,

    /// Delete a template
    Delete {
        /// Template name
        name: String,
    },

    /// Clone a template to create a new volume
    Clone {
        /// Template name
        template: String,

        /// New volume name
        name: String,

        /// Size in GB (optional, defaults to template size)
        #[arg(short, long)]
        size: Option<u64>,
    },
}

#[derive(Subcommand)]
enum NetworkCommands {
    /// List all networks
    List,

    /// Create a new network
    Create {
        /// Network name
        name: String,

        /// IPv4 subnet (CIDR notation, e.g., "10.0.0.0/24")
        #[arg(long)]
        ipv4_subnet: Option<String>,

        /// IPv6 prefix (CIDR notation, e.g., "fd00::/64")
        #[arg(long)]
        ipv6_prefix: Option<String>,

        /// DNS servers (comma-separated)
        #[arg(long)]
        dns: Option<String>,

        /// Make this a public network (enables internet access)
        #[arg(long)]
        public: bool,
    },

    /// Get network details
    Get {
        /// Network ID or name
        id: String,
    },

    /// Delete a network
    Delete {
        /// Network ID or name
        id: String,

        /// Force delete even if NICs exist
        #[arg(short, long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum NicCommands {
    /// List all NICs
    List {
        /// Filter by network ID or name
        #[arg(short, long)]
        network: Option<String>,
    },

    /// Create a new NIC
    Create {
        /// Network ID or name
        network: String,

        /// NIC name (optional)
        #[arg(short, long)]
        name: Option<String>,

        /// MAC address (auto-generated if not specified)
        #[arg(long)]
        mac: Option<String>,

        /// IPv4 address (auto-allocated if not specified)
        #[arg(long)]
        ipv4: Option<String>,

        /// IPv6 address (auto-allocated if not specified)
        #[arg(long)]
        ipv6: Option<String>,
    },

    /// Get NIC details
    Get {
        /// NIC ID or name
        id: String,
    },

    /// Delete a NIC
    Delete {
        /// NIC ID or name
        id: String,
    },
}

#[derive(Subcommand)]
enum PodCommands {
    /// Run a new pod (detached)
    Run {
        /// Pod name
        #[arg(long)]
        name: Option<String>,

        /// Number of vCPUs
        #[arg(long, default_value = "1")]
        cpus: u32,

        /// Memory size (e.g., 256M, 1G)
        #[arg(long, default_value = "256M")]
        memory: String,

        /// Root disk size (e.g., 4G, 10G)
        #[arg(long, default_value = "4G")]
        disk: String,

        /// Environment variables
        #[arg(short, long, value_name = "KEY=VAL")]
        env: Vec<String>,

        /// Network name
        #[arg(long)]
        net: Option<String>,

        /// Container image
        image: String,

        /// Command and arguments
        #[arg(last = true)]
        command: Vec<String>,
    },

    /// List pods
    Ps,

    /// Stop a pod
    Stop {
        /// Pod name or ID
        name_or_id: String,

        /// Timeout in seconds
        #[arg(short, long, default_value = "10")]
        timeout: u32,
    },

    /// Remove a pod (and its volume/nic)
    Rm {
        /// Pod name or ID
        name_or_id: String,

        /// Force remove running pod
        #[arg(short, long)]
        force: bool,
    },

    /// Attach to pod console
    Attach {
        /// Pod name or ID
        name_or_id: String,
    },
}

#[derive(Tabled)]
struct VmRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "NAME")]
    name: String,
    #[tabled(rename = "STATE")]
    state: String,
    #[tabled(rename = "VCPUS")]
    vcpus: u32,
    #[tabled(rename = "MEMORY")]
    memory: String,
}

impl From<Vm> for VmRow {
    fn from(vm: Vm) -> Self {
        let state = format_state(vm.state());
        let config = vm.config.unwrap_or_default();
        Self {
            id: vm.id,
            name: vm.name.unwrap_or_else(|| "-".to_string()),
            state,
            vcpus: config.vcpus,
            memory: format!("{}MB", config.memory_mb),
        }
    }
}

fn format_state(state: VmState) -> String {
    match state {
        VmState::Unspecified => "unknown".to_string(),
        VmState::Stopped => "stopped".to_string(),
        VmState::Starting => "starting".to_string(),
        VmState::Running => "running".to_string(),
        VmState::Stopping => "stopping".to_string(),
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.1} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn format_pod_state(state: PodState) -> String {
    match state {
        PodState::Unspecified => "unknown".to_string(),
        PodState::Created => "created".to_string(),
        PodState::Starting => "starting".to_string(),
        PodState::Running => "running".to_string(),
        PodState::Stopping => "stopping".to_string(),
        PodState::Stopped => "stopped".to_string(),
        PodState::Failed => "failed".to_string(),
    }
}

/// Parse size string like "4G", "256M", "1024K" to bytes
fn parse_size(s: &str) -> Result<u64, Box<dyn std::error::Error>> {
    let s = s.trim().to_uppercase();
    let (num_str, multiplier) = if s.ends_with('G') {
        (&s[..s.len() - 1], 1024 * 1024 * 1024)
    } else if s.ends_with('M') {
        (&s[..s.len() - 1], 1024 * 1024)
    } else if s.ends_with('K') {
        (&s[..s.len() - 1], 1024)
    } else {
        (s.as_str(), 1)
    };
    let num: u64 = num_str.parse()?;
    Ok(num * multiplier)
}

/// Parse memory string like "256M", "1G" to megabytes
fn parse_memory_mb(s: &str) -> Result<u64, Box<dyn std::error::Error>> {
    let bytes = parse_size(s)?;
    Ok(bytes / (1024 * 1024))
}

/// Resolve pod name or ID to pod ID
async fn resolve_pod_id(
    client: &mut PodServiceClient<Channel>,
    name_or_id: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    // First try to get by ID
    if let Ok(response) = client
        .get_pod(GetPodRequest {
            id: name_or_id.to_string(),
        })
        .await
    {
        return Ok(response.into_inner().id);
    }

    // Otherwise search by name
    let pods = client
        .list_pods(ListPodsRequest {})
        .await?
        .into_inner()
        .pods;
    for pod in pods {
        if pod.name == name_or_id {
            return Ok(pod.id);
        }
    }

    Err(format!("Pod '{}' not found", name_or_id).into())
}

/// Resolve VM name or ID to VM ID
async fn resolve_vm_id(
    client: &mut VmServiceClient<Channel>,
    name_or_id: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    // First try to get by ID
    if let Ok(response) = client
        .get_vm(GetVmRequest {
            id: name_or_id.to_string(),
        })
        .await
    {
        return Ok(response.into_inner().id);
    }

    // Otherwise search by name
    let vms = client.list_vms(ListVmsRequest {}).await?.into_inner().vms;
    for vm in vms {
        if vm.name.as_deref() == Some(name_or_id) {
            return Ok(vm.id);
        }
    }

    Err(format!("VM '{}' not found", name_or_id).into())
}

async fn run_console(
    client: &mut VmServiceClient<Channel>,
    vm_id: String,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Connecting to console... (press Ctrl+a t to exit)");

    // Create channel for input
    let (tx, rx) = tokio::sync::mpsc::channel::<ConsoleInput>(32);

    // Send initial message with VM ID
    tx.send(ConsoleInput {
        vm_id: vm_id.clone(),
        data: vec![],
    })
    .await?;

    // Start console stream
    let input_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    let response = client.console(input_stream).await?;
    let mut output_stream = response.into_inner();

    // Enable raw mode
    enable_raw_mode()?;

    // Spawn task to read from stdin
    let tx_clone = tx.clone();
    let stdin_task = tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 1];
        let mut saw_ctrl_a = false;

        loop {
            match stdin.read(&mut buf).await {
                Ok(0) => break,
                Ok(_) => {
                    // Ctrl+a (0x01) followed by 't' to exit
                    if buf[0] == 0x01 {
                        saw_ctrl_a = true;
                        continue;
                    }

                    if saw_ctrl_a {
                        saw_ctrl_a = false;
                        if buf[0] == b't' {
                            break;
                        }
                        // Send the Ctrl+a we held back, then continue with current char
                        if tx_clone
                            .send(ConsoleInput {
                                vm_id: String::new(),
                                data: vec![0x01],
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }

                    if tx_clone
                        .send(ConsoleInput {
                            vm_id: String::new(),
                            data: buf.to_vec(),
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Read from output stream and write to stdout
    let output_task = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(result) = output_stream.next().await {
            match result {
                Ok(output) => {
                    if stdout.write_all(&output.data).await.is_err() {
                        break;
                    }
                    let _ = stdout.flush().await;
                }
                Err(_) => break,
            }
        }
    });

    // Wait for either task to complete
    tokio::select! {
        _ = stdin_task => {}
        _ = output_task => {}
    }

    // Disable raw mode
    disable_raw_mode()?;
    println!("\nDisconnected from console.");

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Try to connect to mvirt-vmm (optional for TUI - required for subcommands)
    let vm_client = VmServiceClient::connect(cli.server.clone()).await.ok();

    // Try to connect to mvirt-zfs (optional - doesn't fail if unavailable)
    let zfs_client = ZfsServiceClient::connect(cli.zfs_server.clone()).await.ok();

    // Try to connect to mvirt-log (optional - doesn't fail if unavailable)
    let log_client = LogServiceClient::connect(cli.log_server.clone()).await.ok();

    // Try to connect to mvirt-net (optional - doesn't fail if unavailable)
    let mut net_client = NetServiceClient::connect(cli.net_server.clone()).await.ok();

    let Some(command) = cli.command else {
        // No subcommand: start TUI (works even without connections)
        tui::run(vm_client, zfs_client, log_client, net_client).await?;
        return Ok(());
    };

    // Handle network commands (require net_client)
    let is_network_command = matches!(&command, Commands::Network(_) | Commands::Nic(_));

    if is_network_command {
        let Some(mut net_client) = net_client else {
            eprintln!("Error: Cannot connect to mvirt-net at {}", cli.net_server);
            std::process::exit(1);
        };

        match &command {
            Commands::Network(cmd) => match cmd {
                NetworkCommands::List => {
                    let response = net_client
                        .list_networks(net_proto::ListNetworksRequest {})
                        .await?;
                    let networks = response.into_inner().networks;
                    if networks.is_empty() {
                        println!("No networks found");
                    } else {
                        println!(
                            "{:<36} {:<15} {:<18} {:>5} {:<6}",
                            "ID", "NAME", "SUBNET", "NICS", "PUBLIC"
                        );
                        for net in networks {
                            let subnet = if !net.ipv4_subnet.is_empty() {
                                net.ipv4_subnet.clone()
                            } else if !net.ipv6_prefix.is_empty() {
                                net.ipv6_prefix.clone()
                            } else {
                                "-".to_string()
                            };
                            println!(
                                "{:<36} {:<15} {:<18} {:>5} {:<6}",
                                net.id,
                                net.name,
                                subnet,
                                net.nic_count,
                                if net.is_public { "yes" } else { "no" }
                            );
                        }
                    }
                }
                NetworkCommands::Create {
                    name,
                    ipv4_subnet,
                    ipv6_prefix,
                    dns,
                    public,
                } => {
                    let ipv4_enabled = ipv4_subnet.is_some();
                    let ipv6_enabled = ipv6_prefix.is_some();

                    if !ipv4_enabled && !ipv6_enabled {
                        eprintln!(
                            "Error: At least one of --ipv4-subnet or --ipv6-prefix is required"
                        );
                        std::process::exit(1);
                    }

                    let dns_servers: Vec<String> = dns
                        .as_ref()
                        .map(|s| s.split(',').map(|s| s.trim().to_string()).collect())
                        .unwrap_or_default();

                    let response = net_client
                        .create_network(net_proto::CreateNetworkRequest {
                            name: name.clone(),
                            ipv4_enabled,
                            ipv4_subnet: ipv4_subnet.clone().unwrap_or_default(),
                            ipv6_enabled,
                            ipv6_prefix: ipv6_prefix.clone().unwrap_or_default(),
                            dns_servers,
                            ntp_servers: vec![],
                            is_public: *public,
                        })
                        .await?;
                    let net = response.into_inner();
                    println!("Created network: {} ({})", net.name, net.id);
                }
                NetworkCommands::Get { id } => {
                    let response = net_client
                        .get_network(net_proto::GetNetworkRequest {
                            identifier: Some(net_proto::get_network_request::Identifier::Name(
                                id.clone(),
                            )),
                        })
                        .await?;
                    let net = response.into_inner();
                    println!("ID:       {}", net.id);
                    println!("Name:     {}", net.name);
                    println!("Public:   {}", if net.is_public { "yes" } else { "no" });
                    if net.ipv4_enabled {
                        println!("IPv4:     {}", net.ipv4_subnet);
                    }
                    if net.ipv6_enabled {
                        println!("IPv6:     {}", net.ipv6_prefix);
                    }
                    if !net.dns_servers.is_empty() {
                        println!("DNS:      {}", net.dns_servers.join(", "));
                    }
                    println!("NICs:     {}", net.nic_count);
                    println!("Created:  {}", net.created_at);
                }
                NetworkCommands::Delete { id, force } => {
                    let response = net_client
                        .delete_network(net_proto::DeleteNetworkRequest {
                            id: id.clone(),
                            force: *force,
                        })
                        .await?;
                    let resp = response.into_inner();
                    if resp.deleted {
                        if resp.nics_deleted > 0 {
                            println!(
                                "Deleted network {} ({} NICs also deleted)",
                                id, resp.nics_deleted
                            );
                        } else {
                            println!("Deleted network: {}", id);
                        }
                    }
                }
            },

            Commands::Nic(cmd) => match cmd {
                NicCommands::List { network } => {
                    let response = net_client
                        .list_nics(net_proto::ListNicsRequest {
                            network_id: network.clone().unwrap_or_default(),
                        })
                        .await?;
                    let nics = response.into_inner().nics;
                    if nics.is_empty() {
                        println!("No NICs found");
                    } else {
                        println!(
                            "{:<36} {:<15} {:<17} {:<15} {:<8}",
                            "ID", "NAME", "MAC", "IPv4", "STATE"
                        );
                        for nic in nics {
                            let state = match net_proto::NicState::try_from(nic.state) {
                                Ok(net_proto::NicState::Created) => "created",
                                Ok(net_proto::NicState::Active) => "active",
                                Ok(net_proto::NicState::Error) => "error",
                                _ => "unknown",
                            };
                            println!(
                                "{:<36} {:<15} {:<17} {:<15} {:<8}",
                                nic.id,
                                if nic.name.is_empty() { "-" } else { &nic.name },
                                nic.mac_address,
                                if nic.ipv4_address.is_empty() {
                                    "-"
                                } else {
                                    &nic.ipv4_address
                                },
                                state
                            );
                        }
                    }
                }
                NicCommands::Create {
                    network,
                    name,
                    mac,
                    ipv4,
                    ipv6,
                } => {
                    let response = net_client
                        .create_nic(net_proto::CreateNicRequest {
                            network_id: network.clone(),
                            name: name.clone().unwrap_or_default(),
                            mac_address: mac.clone().unwrap_or_default(),
                            ipv4_address: ipv4.clone().unwrap_or_default(),
                            ipv6_address: ipv6.clone().unwrap_or_default(),
                            routed_ipv4_prefixes: vec![],
                            routed_ipv6_prefixes: vec![],
                        })
                        .await?;
                    let nic = response.into_inner();
                    println!("Created NIC: {} ({})", nic.id, nic.mac_address);
                    println!("  Socket: {}", nic.socket_path);
                    if !nic.ipv4_address.is_empty() {
                        println!("  IPv4:   {}", nic.ipv4_address);
                    }
                    if !nic.ipv6_address.is_empty() {
                        println!("  IPv6:   {}", nic.ipv6_address);
                    }
                }
                NicCommands::Get { id } => {
                    let response = net_client
                        .get_nic(net_proto::GetNicRequest {
                            identifier: Some(net_proto::get_nic_request::Identifier::Name(
                                id.clone(),
                            )),
                        })
                        .await?;
                    let nic = response.into_inner();
                    let state = match net_proto::NicState::try_from(nic.state) {
                        Ok(net_proto::NicState::Created) => "created",
                        Ok(net_proto::NicState::Active) => "active",
                        Ok(net_proto::NicState::Error) => "error",
                        _ => "unknown",
                    };
                    println!("ID:       {}", nic.id);
                    println!(
                        "Name:     {}",
                        if nic.name.is_empty() { "-" } else { &nic.name }
                    );
                    println!("Network:  {}", nic.network_id);
                    println!("MAC:      {}", nic.mac_address);
                    println!("State:    {}", state);
                    println!("Socket:   {}", nic.socket_path);
                    if !nic.ipv4_address.is_empty() {
                        println!("IPv4:     {}", nic.ipv4_address);
                    }
                    if !nic.ipv6_address.is_empty() {
                        println!("IPv6:     {}", nic.ipv6_address);
                    }
                    println!("Created:  {}", nic.created_at);
                }
                NicCommands::Delete { id } => {
                    net_client
                        .delete_nic(net_proto::DeleteNicRequest { id: id.clone() })
                        .await?;
                    println!("Deleted NIC: {}", id);
                }
            },

            _ => unreachable!(),
        }

        return Ok(());
    }

    // Handle pod commands (require zfs, net, and pod clients)
    if let Commands::Pod(ref pod_cmd) = command {
        let Some(mut zfs_client) = zfs_client else {
            eprintln!("Error: Cannot connect to mvirt-zfs at {}", cli.zfs_server);
            std::process::exit(1);
        };

        let mut pod_client = PodServiceClient::connect(cli.server.clone())
            .await
            .map_err(|e| format!("Cannot connect to mvirt-vmm: {}", e))?;

        match pod_cmd {
            PodCommands::Run {
                name,
                cpus,
                memory,
                disk,
                env,
                net,
                image,
                command: cmd_args,
            } => {
                let pod_name = name
                    .clone()
                    .unwrap_or_else(|| format!("pod-{}", &uuid::Uuid::new_v4().to_string()[..8]));

                // 1. Parse sizes
                let disk_bytes = parse_size(disk)?;
                let memory_mb = parse_memory_mb(memory)?;

                // 2. Create ZFS volume
                let volume_name = format!("{}-root", pod_name);
                let volume = zfs_client
                    .create_volume(zfs_proto::CreateVolumeRequest {
                        name: volume_name.clone(),
                        size_bytes: disk_bytes,
                        volblocksize: None,
                    })
                    .await?;
                let volume_path = volume.into_inner().path;

                // 3. Create NIC if network specified
                let (nic_socket_path, nic_id) = if let Some(net_name) = net {
                    let Some(ref mut net_client) = net_client else {
                        eprintln!("Error: Cannot connect to mvirt-net at {}", cli.net_server);
                        let _ = zfs_client
                            .delete_volume(zfs_proto::DeleteVolumeRequest {
                                name: volume_name.clone(),
                            })
                            .await;
                        std::process::exit(1);
                    };

                    // Lookup network by name
                    let networks = match net_client
                        .list_networks(net_proto::ListNetworksRequest {})
                        .await
                    {
                        Ok(resp) => resp.into_inner().networks,
                        Err(e) => {
                            eprintln!("Error: Failed to list networks: {}", e);
                            let _ = zfs_client
                                .delete_volume(zfs_proto::DeleteVolumeRequest {
                                    name: volume_name.clone(),
                                })
                                .await;
                            std::process::exit(1);
                        }
                    };
                    let network = match networks
                        .iter()
                        .find(|n| n.name == *net_name || n.id == *net_name)
                    {
                        Some(n) => n,
                        None => {
                            eprintln!("Error: Network '{}' not found", net_name);
                            let _ = zfs_client
                                .delete_volume(zfs_proto::DeleteVolumeRequest {
                                    name: volume_name.clone(),
                                })
                                .await;
                            std::process::exit(1);
                        }
                    };

                    // Create NIC
                    let nic = match net_client
                        .create_nic(net_proto::CreateNicRequest {
                            network_id: network.id.clone(),
                            name: format!("{}-nic", pod_name),
                            mac_address: String::new(),
                            ipv4_address: String::new(),
                            ipv6_address: String::new(),
                            routed_ipv4_prefixes: vec![],
                            routed_ipv6_prefixes: vec![],
                        })
                        .await
                    {
                        Ok(resp) => resp.into_inner(),
                        Err(e) => {
                            eprintln!("Error: Failed to create NIC: {}", e);
                            let _ = zfs_client
                                .delete_volume(zfs_proto::DeleteVolumeRequest {
                                    name: volume_name.clone(),
                                })
                                .await;
                            std::process::exit(1);
                        }
                    };

                    (Some(nic.socket_path), Some(nic.id))
                } else {
                    (None, None)
                };

                // 4. Build container spec
                let container_spec = ContainerSpec {
                    id: String::new(),
                    name: String::new(),
                    image: image.clone(),
                    command: if cmd_args.is_empty() {
                        vec![]
                    } else {
                        vec![cmd_args[0].clone()]
                    },
                    args: cmd_args.iter().skip(1).cloned().collect(),
                    env: env.clone(),
                    working_dir: String::new(),
                };

                // 5. Create pod
                let pod = match pod_client
                    .create_pod(CreatePodRequest {
                        name: Some(pod_name.clone()),
                        containers: vec![container_spec],
                        resources: Some(PodResources {
                            vcpus: *cpus,
                            memory_mb: memory_mb as u64,
                            disk_size_gb: disk_bytes / (1024 * 1024 * 1024),
                        }),
                        root_disk_path: Some(volume_path),
                        nic_socket_path,
                    })
                    .await
                {
                    Ok(resp) => resp.into_inner(),
                    Err(e) => {
                        eprintln!("Error: Failed to create pod: {}", e);
                        let _ = zfs_client
                            .delete_volume(zfs_proto::DeleteVolumeRequest {
                                name: volume_name.clone(),
                            })
                            .await;
                        if let (Some(net_client), Some(id)) = (&mut net_client, nic_id) {
                            let _ = net_client
                                .delete_nic(net_proto::DeleteNicRequest { id })
                                .await;
                        }
                        std::process::exit(1);
                    }
                };

                // 6. Start pod
                let pod = match pod_client
                    .start_pod(StartPodRequest { id: pod.id.clone() })
                    .await
                {
                    Ok(resp) => resp.into_inner(),
                    Err(e) => {
                        eprintln!("Error: Failed to start pod: {}", e);
                        // Delete the created pod
                        let _ = pod_client
                            .delete_pod(DeletePodRequest {
                                id: pod.id.clone(),
                                force: true,
                            })
                            .await;
                        let _ = zfs_client
                            .delete_volume(zfs_proto::DeleteVolumeRequest {
                                name: volume_name.clone(),
                            })
                            .await;
                        if let (Some(net_client), Some(id)) = (&mut net_client, nic_id) {
                            let _ = net_client
                                .delete_nic(net_proto::DeleteNicRequest { id })
                                .await;
                        }
                        std::process::exit(1);
                    }
                };

                // 7. Output pod ID
                println!("{}", pod.id);
            }

            PodCommands::Ps => {
                let response = pod_client.list_pods(ListPodsRequest {}).await?;
                let pods = response.into_inner().pods;

                if pods.is_empty() {
                    println!("No pods found");
                } else {
                    println!(
                        "{:<36} {:<15} {:<10} {:<15} {:<20}",
                        "ID", "NAME", "STATE", "IP", "IMAGE"
                    );
                    for pod in pods {
                        let state = format_pod_state(
                            PodState::try_from(pod.state).unwrap_or(PodState::Unspecified),
                        );
                        let image = pod
                            .containers
                            .first()
                            .map(|c| c.image.as_str())
                            .unwrap_or("-");
                        println!(
                            "{:<36} {:<15} {:<10} {:<15} {:<20}",
                            pod.id,
                            pod.name,
                            state,
                            if pod.ip_address.is_empty() {
                                "-"
                            } else {
                                &pod.ip_address
                            },
                            image
                        );
                    }
                }
            }

            PodCommands::Stop {
                name_or_id,
                timeout,
            } => {
                // Find pod by name or ID
                let pod_id = resolve_pod_id(&mut pod_client, name_or_id).await?;
                pod_client
                    .stop_pod(StopPodRequest {
                        id: pod_id.clone(),
                        timeout_seconds: *timeout,
                    })
                    .await?;
                println!("Stopped pod: {}", pod_id);
            }

            PodCommands::Rm { name_or_id, force } => {
                // Find pod by name or ID
                let pod_id = resolve_pod_id(&mut pod_client, name_or_id).await?;

                // Get pod info to find volume name
                let pod = pod_client
                    .get_pod(GetPodRequest { id: pod_id.clone() })
                    .await?
                    .into_inner();

                // Delete pod from VMM
                pod_client
                    .delete_pod(DeletePodRequest {
                        id: pod_id.clone(),
                        force: *force,
                    })
                    .await?;

                // Delete ZFS volume (volume name is pod-name-root)
                let volume_name = format!("{}-root", pod.name);
                if let Err(e) = zfs_client
                    .delete_volume(zfs_proto::DeleteVolumeRequest {
                        name: volume_name.clone(),
                    })
                    .await
                {
                    eprintln!("Warning: Failed to delete volume {}: {}", volume_name, e);
                }

                // TODO: Delete NIC if exists (need to track NIC ID in pod)

                println!("Removed pod: {}", pod_id);
            }

            PodCommands::Attach { name_or_id } => {
                // Find pod by name or ID
                let pod_id = resolve_pod_id(&mut pod_client, name_or_id).await?;

                // Get pod to find VM ID
                let pod = pod_client
                    .get_pod(GetPodRequest { id: pod_id.clone() })
                    .await?
                    .into_inner();

                if pod.vm_id.is_empty() {
                    eprintln!("Error: Pod is not running");
                    std::process::exit(1);
                }

                // Connect to VM console
                let mut vm_client = VmServiceClient::connect(cli.server.clone()).await?;
                run_console(&mut vm_client, pod.vm_id).await?;
            }
        }

        return Ok(());
    }

    // Handle storage commands (require zfs_client, not vm_client)
    let is_storage_command = matches!(
        &command,
        Commands::Import { .. }
            | Commands::Pool
            | Commands::Volume(_)
            | Commands::Snapshot(_)
            | Commands::Template(_)
    );

    if is_storage_command {
        let Some(mut zfs_client) = zfs_client else {
            eprintln!("Error: Cannot connect to mvirt-zfs at {}", cli.zfs_server);
            std::process::exit(1);
        };

        match &command {
            Commands::Import { name, source } => {
                // Start import
                let response = zfs_client
                    .import_template(zfs_proto::ImportTemplateRequest {
                        name: name.clone(),
                        source: source.clone(),
                        size_bytes: None,
                    })
                    .await?;

                let job = response.into_inner();
                println!("Import started: {} (job {})", name, job.id);

                // Poll for completion
                loop {
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

                    let status = zfs_client
                        .get_import_job(zfs_proto::GetImportJobRequest { id: job.id.clone() })
                        .await?
                        .into_inner();

                    let state = zfs_proto::ImportJobState::try_from(status.state)
                        .unwrap_or(zfs_proto::ImportJobState::Unspecified);

                    let progress = if status.total_bytes > 0 {
                        format!(
                            "{:.1}%",
                            status.bytes_written as f64 / status.total_bytes as f64 * 100.0
                        )
                    } else {
                        format!("{} bytes", status.bytes_written)
                    };

                    println!("  {:?}: {}", state, progress);

                    match state {
                        zfs_proto::ImportJobState::Completed => {
                            if let Some(template) = status.template {
                                println!("Import completed: {} ({})", template.name, template.id);
                            } else {
                                println!("Import completed!");
                            }
                            return Ok(());
                        }
                        zfs_proto::ImportJobState::Failed => {
                            eprintln!("Import failed: {}", status.error.unwrap_or_default());
                            std::process::exit(1);
                        }
                        zfs_proto::ImportJobState::Cancelled => {
                            eprintln!("Import cancelled");
                            std::process::exit(1);
                        }
                        _ => continue,
                    }
                }
            }

            Commands::Pool => {
                let stats = zfs_client
                    .get_pool_stats(zfs_proto::GetPoolStatsRequest {})
                    .await?
                    .into_inner();
                println!("Pool: {}", stats.name);
                println!("  Total:       {:>10}", format_bytes(stats.total_bytes));
                println!("  Used:        {:>10}", format_bytes(stats.used_bytes));
                println!("  Available:   {:>10}", format_bytes(stats.available_bytes));
                println!(
                    "  Provisioned: {:>10}",
                    format_bytes(stats.provisioned_bytes)
                );
                if stats.compression_ratio > 1.0 {
                    println!("  Compression: {:.2}x", stats.compression_ratio);
                }
            }

            Commands::Volume(cmd) => match cmd {
                VolumeCommands::List => {
                    let response = zfs_client
                        .list_volumes(zfs_proto::ListVolumesRequest {})
                        .await?;
                    let volumes = response.into_inner().volumes;
                    if volumes.is_empty() {
                        println!("No volumes found");
                    } else {
                        println!("{:<36} {:<20} {:>10} {:>10}", "ID", "NAME", "SIZE", "USED");
                        for vol in volumes {
                            println!(
                                "{:<36} {:<20} {:>10} {:>10}",
                                vol.id,
                                vol.name,
                                format_bytes(vol.volsize_bytes),
                                format_bytes(vol.used_bytes)
                            );
                        }
                    }
                }
                VolumeCommands::Create { name, size } => {
                    let size_bytes = size * 1024 * 1024 * 1024;
                    let response = zfs_client
                        .create_volume(zfs_proto::CreateVolumeRequest {
                            name: name.clone(),
                            size_bytes,
                            volblocksize: None,
                        })
                        .await?;
                    let vol = response.into_inner();
                    println!("Created volume: {} ({})", vol.name, vol.id);
                }
                VolumeCommands::Delete { name } => {
                    zfs_client
                        .delete_volume(zfs_proto::DeleteVolumeRequest { name: name.clone() })
                        .await?;
                    println!("Deleted volume: {}", name);
                }
                VolumeCommands::Resize { name, size } => {
                    let new_size_bytes = size * 1024 * 1024 * 1024;
                    let response = zfs_client
                        .resize_volume(zfs_proto::ResizeVolumeRequest {
                            name: name.clone(),
                            new_size_bytes,
                        })
                        .await?;
                    let vol = response.into_inner();
                    println!(
                        "Resized volume: {} to {}",
                        vol.name,
                        format_bytes(vol.volsize_bytes)
                    );
                }
            },

            Commands::Snapshot(cmd) => match cmd {
                SnapshotCommands::List { volume } => {
                    let response = zfs_client
                        .list_snapshots(zfs_proto::ListSnapshotsRequest {
                            volume_name: volume.clone(),
                        })
                        .await?;
                    let snapshots = response.into_inner().snapshots;
                    if snapshots.is_empty() {
                        println!("No snapshots found for volume: {}", volume);
                    } else {
                        println!("{:<36} {:<20} {:>10}", "ID", "NAME", "USED");
                        for snap in snapshots {
                            println!(
                                "{:<36} {:<20} {:>10}",
                                snap.id,
                                snap.name,
                                format_bytes(snap.used_bytes)
                            );
                        }
                    }
                }
                SnapshotCommands::Create { volume, name } => {
                    let response = zfs_client
                        .create_snapshot(zfs_proto::CreateSnapshotRequest {
                            volume_name: volume.clone(),
                            snapshot_name: name.clone(),
                        })
                        .await?;
                    let snap = response.into_inner();
                    println!("Created snapshot: {}@{} ({})", volume, snap.name, snap.id);
                }
                SnapshotCommands::Delete { volume, name } => {
                    zfs_client
                        .delete_snapshot(zfs_proto::DeleteSnapshotRequest {
                            volume_name: volume.clone(),
                            snapshot_name: name.clone(),
                        })
                        .await?;
                    println!("Deleted snapshot: {}@{}", volume, name);
                }
                SnapshotCommands::Rollback { volume, name } => {
                    let response = zfs_client
                        .rollback_snapshot(zfs_proto::RollbackSnapshotRequest {
                            volume_name: volume.clone(),
                            snapshot_name: name.clone(),
                        })
                        .await?;
                    let vol = response.into_inner();
                    println!("Rolled back volume {} to snapshot {}", vol.name, name);
                }
                SnapshotCommands::Promote {
                    volume,
                    snapshot,
                    template,
                } => {
                    let response = zfs_client
                        .promote_snapshot_to_template(zfs_proto::PromoteSnapshotRequest {
                            volume_name: volume.clone(),
                            snapshot_name: snapshot.clone(),
                            template_name: template.clone(),
                        })
                        .await?;
                    let tpl = response.into_inner();
                    println!(
                        "Promoted {}@{} to template '{}' ({})",
                        volume, snapshot, tpl.name, tpl.id
                    );
                }
            },

            Commands::Template(cmd) => match cmd {
                TemplateCommands::List => {
                    let response = zfs_client
                        .list_templates(zfs_proto::ListTemplatesRequest {})
                        .await?;
                    let templates = response.into_inner().templates;
                    if templates.is_empty() {
                        println!("No templates found");
                    } else {
                        println!("{:<36} {:<20} {:>10}", "ID", "NAME", "SIZE");
                        for tpl in templates {
                            println!(
                                "{:<36} {:<20} {:>10}",
                                tpl.id,
                                tpl.name,
                                format_bytes(tpl.size_bytes)
                            );
                        }
                    }
                }
                TemplateCommands::Delete { name } => {
                    zfs_client
                        .delete_template(zfs_proto::DeleteTemplateRequest { name: name.clone() })
                        .await?;
                    println!("Deleted template: {}", name);
                }
                TemplateCommands::Clone {
                    template,
                    name,
                    size,
                } => {
                    let size_bytes = size.map(|s| s * 1024 * 1024 * 1024);
                    let response = zfs_client
                        .clone_from_template(zfs_proto::CloneFromTemplateRequest {
                            template_name: template.clone(),
                            new_volume_name: name.clone(),
                            size_bytes,
                        })
                        .await?;
                    let vol = response.into_inner();
                    println!(
                        "Cloned template {} to volume {} ({})",
                        template, vol.name, vol.id
                    );
                }
            },

            _ => unreachable!(),
        }

        return Ok(());
    }

    // Other subcommands require a connection to the VMM
    let Some(mut client) = vm_client else {
        eprintln!("Error: Cannot connect to mvirt daemon at {}", cli.server);
        std::process::exit(1);
    };

    match command {
        Commands::Create {
            name,
            vcpus,
            memory,
            boot,
            kernel,
            initramfs,
            cmdline,
            disk,
            user_data,
            nested_virt,
        } => {
            // Parse boot mode
            let boot_mode = match boot.to_lowercase().as_str() {
                "disk" => BootMode::Disk,
                "kernel" => BootMode::Kernel,
                _ => {
                    eprintln!(
                        "Error: Invalid boot mode '{}'. Use 'disk' or 'kernel'.",
                        boot
                    );
                    std::process::exit(1);
                }
            };

            // Validate boot mode requirements
            if boot_mode == BootMode::Kernel && kernel.is_none() {
                eprintln!("Error: Kernel boot mode requires --kernel");
                std::process::exit(1);
            }
            if boot_mode == BootMode::Disk && disk.is_none() {
                eprintln!("Error: Disk boot mode requires --disk");
                std::process::exit(1);
            }

            let disks = disk
                .map(|path| {
                    vec![DiskConfig {
                        path,
                        readonly: false,
                    }]
                })
                .unwrap_or_default();

            // Read user-data file if provided
            let user_data_content = match user_data {
                Some(path) => Some(std::fs::read_to_string(&path).map_err(|e| {
                    format!("Failed to read user-data file {}: {}", path.display(), e)
                })?),
                None => None,
            };

            let request = CreateVmRequest {
                name,
                config: Some(VmConfig {
                    vcpus,
                    memory_mb: memory,
                    boot_mode: boot_mode.into(),
                    kernel,
                    initramfs,
                    cmdline,
                    disks,
                    nics: vec![],
                    user_data: user_data_content,
                    nested_virt,
                }),
            };

            let response = client.create_vm(request).await?;
            let vm = response.into_inner();
            println!("Created VM: {}", vm.id);
        }

        Commands::List => {
            let response = client.list_vms(ListVmsRequest {}).await?;
            let vms = response.into_inner().vms;

            if vms.is_empty() {
                println!("No VMs found");
            } else {
                let rows: Vec<VmRow> = vms.into_iter().map(VmRow::from).collect();
                let table = Table::new(rows);
                println!("{}", table);
            }
        }

        Commands::Get { id } => {
            let response = client.get_vm(GetVmRequest { id }).await?;
            let vm = response.into_inner();
            let config = vm.config.as_ref().unwrap();

            println!("ID:      {}", vm.id);
            println!("Name:    {}", vm.name.as_deref().unwrap_or("-"));
            println!("State:   {}", format_state(vm.state()));
            println!("vCPUs:   {}", config.vcpus);
            println!("Memory:  {}MB", config.memory_mb);
            let boot_mode_str = match BootMode::try_from(config.boot_mode) {
                Ok(BootMode::Disk) | Ok(BootMode::Unspecified) | Err(_) => "disk",
                Ok(BootMode::Kernel) => "kernel",
            };
            println!("Boot:    {}", boot_mode_str);
            if let Some(kernel) = &config.kernel {
                println!("Kernel:  {}", kernel);
            }
            if let Some(initramfs) = &config.initramfs {
                println!("Initramfs: {}", initramfs);
            }
            if let Some(cmdline) = &config.cmdline {
                println!("Cmdline: {}", cmdline);
            }
            if !config.disks.is_empty() {
                println!("Disks:");
                for disk in &config.disks {
                    println!("  - {} (ro: {})", disk.path, disk.readonly);
                }
            }
        }

        Commands::Delete { id } => {
            let vm_id = resolve_vm_id(&mut client, &id).await?;
            client
                .delete_vm(DeleteVmRequest { id: vm_id.clone() })
                .await?;
            println!("Deleted VM: {}", vm_id);
        }

        Commands::Start { id } => {
            let vm_id = resolve_vm_id(&mut client, &id).await?;
            let response = client.start_vm(StartVmRequest { id: vm_id }).await?;
            let vm = response.into_inner();
            println!(
                "Started VM: {} (state: {})",
                vm.id,
                format_state(vm.state())
            );
        }

        Commands::Stop { id, timeout } => {
            let vm_id = resolve_vm_id(&mut client, &id).await?;
            let response = client
                .stop_vm(StopVmRequest {
                    id: vm_id,
                    timeout_seconds: timeout,
                })
                .await?;
            let vm = response.into_inner();
            println!(
                "Stopped VM: {} (state: {})",
                vm.id,
                format_state(vm.state())
            );
        }

        Commands::Kill { id } => {
            let vm_id = resolve_vm_id(&mut client, &id).await?;
            let response = client.kill_vm(KillVmRequest { id: vm_id }).await?;
            let vm = response.into_inner();
            println!("Killed VM: {} (state: {})", vm.id, format_state(vm.state()));
        }

        Commands::Console { id } => {
            let vm_id = resolve_vm_id(&mut client, &id).await?;
            run_console(&mut client, vm_id).await?;
        }

        Commands::Import { .. }
        | Commands::Pool
        | Commands::Volume(_)
        | Commands::Snapshot(_)
        | Commands::Template(_)
        | Commands::Network(_)
        | Commands::Nic(_)
        | Commands::Pod(_) => {
            // Handled above
            unreachable!()
        }
    }

    Ok(())
}
