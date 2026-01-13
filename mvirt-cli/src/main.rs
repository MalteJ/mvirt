use clap::{Parser, Subcommand};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use tabled::{Table, Tabled};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_stream::StreamExt;
use tonic::transport::Channel;

pub mod proto {
    tonic::include_proto!("mvirt");
}

mod tui;

use proto::vm_service_client::VmServiceClient;
use proto::*;

#[derive(Parser)]
#[command(name = "mvirt")]
#[command(about = "CLI for mvirt VM manager", long_about = None)]
struct Cli {
    /// gRPC server address
    #[arg(short, long, default_value = "http://[::1]:50051")]
    server: String,

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

async fn connect(server: &str) -> Result<VmServiceClient<Channel>, Box<dyn std::error::Error>> {
    VmServiceClient::connect(server.to_string())
        .await
        .map_err(|e| {
            let err_str = format!("{:?}", e);
            if err_str.contains("ConnectionRefused") || err_str.contains("Connection refused") {
                format!("Cannot connect to mvirt daemon at {}", server).into()
            } else {
                Box::new(e) as Box<dyn std::error::Error>
            }
        })
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
    let client = match connect(&cli.server).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    let Some(command) = cli.command else {
        // No subcommand: start TUI
        tui::run(client).await?;
        return Ok(());
    };

    let mut client = client;
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
            client.delete_vm(DeleteVmRequest { id: id.clone() }).await?;
            println!("Deleted VM: {}", id);
        }

        Commands::Start { id } => {
            let response = client.start_vm(StartVmRequest { id }).await?;
            let vm = response.into_inner();
            println!(
                "Started VM: {} (state: {})",
                vm.id,
                format_state(vm.state())
            );
        }

        Commands::Stop { id, timeout } => {
            let response = client
                .stop_vm(StopVmRequest {
                    id,
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
            let response = client.kill_vm(KillVmRequest { id }).await?;
            let vm = response.into_inner();
            println!("Killed VM: {} (state: {})", vm.id, format_state(vm.state()));
        }

        Commands::Console { id } => {
            run_console(&mut client, id).await?;
        }
    }

    Ok(())
}
