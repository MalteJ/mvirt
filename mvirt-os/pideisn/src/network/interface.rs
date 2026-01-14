use crate::error::NetworkError;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Interface {
    pub name: String,
    pub index: u32,
    pub mac: [u8; 6],
}

pub fn discover_interfaces() -> Result<Vec<Interface>, NetworkError> {
    let mut interfaces = Vec::new();

    let net_dir = Path::new("/sys/class/net");
    if !net_dir.exists() {
        return Err(NetworkError::NoInterface);
    }

    let entries = fs::read_dir(net_dir).map_err(NetworkError::SocketError)?;

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip loopback
        if name == "lo" {
            continue;
        }

        // Check if it's a physical/virtual network device
        let device_path = format!("/sys/class/net/{}/device", name);
        let virtio_path = format!("/sys/class/net/{}/type", name);

        // Accept if it has a device link OR if it's a valid network type
        if !Path::new(&device_path).exists() {
            // Check if it's at least a valid ethernet type (1)
            if let Ok(type_str) = fs::read_to_string(&virtio_path) {
                let net_type: u32 = type_str.trim().parse().unwrap_or(0);
                if net_type != 1 {
                    // Not ethernet
                    continue;
                }
            } else {
                continue;
            }
        }

        let mac = match read_mac_address(&name) {
            Ok(m) => m,
            Err(_) => continue,
        };

        let index = match read_interface_index(&name) {
            Ok(i) => i,
            Err(_) => continue,
        };

        interfaces.push(Interface { name, index, mac });
    }

    Ok(interfaces)
}

fn read_mac_address(name: &str) -> Result<[u8; 6], NetworkError> {
    let path = format!("/sys/class/net/{}/address", name);
    let mac_str = fs::read_to_string(&path).map_err(NetworkError::SocketError)?;

    parse_mac_address(mac_str.trim())
}

fn parse_mac_address(s: &str) -> Result<[u8; 6], NetworkError> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 6 {
        return Err(NetworkError::InvalidPacket(format!(
            "Invalid MAC address: {}",
            s
        )));
    }

    let mut mac = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        mac[i] = u8::from_str_radix(part, 16)
            .map_err(|_| NetworkError::InvalidPacket(format!("Invalid MAC address: {}", s)))?;
    }

    Ok(mac)
}

fn read_interface_index(name: &str) -> Result<u32, NetworkError> {
    let path = format!("/sys/class/net/{}/ifindex", name);
    let index_str = fs::read_to_string(&path).map_err(NetworkError::SocketError)?;

    index_str
        .trim()
        .parse()
        .map_err(|_| NetworkError::InvalidPacket("Invalid interface index".to_string()))
}

pub fn mac_to_eui64(mac: &[u8; 6]) -> [u8; 8] {
    // Insert ff:fe in middle, flip bit 6 of first byte (universal/local bit)
    let modified_first = mac[0] ^ 0x02;

    [
        modified_first,
        mac[1],
        mac[2],
        0xff,
        0xfe,
        mac[3],
        mac[4],
        mac[5],
    ]
}
