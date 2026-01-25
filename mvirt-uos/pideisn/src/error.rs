use std::fmt;
use std::io;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Nix(nix::errno::Errno),
    Mount {
        target: String,
        source: nix::errno::Errno,
    },
    Network(NetworkError),
}

#[derive(Debug)]
pub enum NetworkError {
    NoInterface,
    Timeout,
    InvalidPacket(String),
    SocketError(io::Error),
    NetlinkError(String),
    DhcpNak,
    NoOffer,
    NoAdvertise,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "IO error: {e}"),
            Error::Nix(e) => write!(f, "System error: {e}"),
            Error::Mount { target, source } => write!(f, "Failed to mount {target}: {source}"),
            Error::Network(e) => write!(f, "Network error: {e}"),
        }
    }
}

impl fmt::Display for NetworkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetworkError::NoInterface => write!(f, "No network interface found"),
            NetworkError::Timeout => write!(f, "Operation timed out"),
            NetworkError::InvalidPacket(msg) => write!(f, "Invalid packet: {msg}"),
            NetworkError::SocketError(e) => write!(f, "Socket error: {e}"),
            NetworkError::NetlinkError(msg) => write!(f, "Netlink error: {msg}"),
            NetworkError::DhcpNak => write!(f, "DHCP NAK received"),
            NetworkError::NoOffer => write!(f, "No DHCP offer received"),
            NetworkError::NoAdvertise => write!(f, "No DHCPv6 advertise received"),
        }
    }
}

impl std::error::Error for Error {}
impl std::error::Error for NetworkError {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<nix::errno::Errno> for Error {
    fn from(e: nix::errno::Errno) -> Self {
        Error::Nix(e)
    }
}

impl From<NetworkError> for Error {
    fn from(e: NetworkError) -> Self {
        Error::Network(e)
    }
}

impl From<io::Error> for NetworkError {
    fn from(e: io::Error) -> Self {
        NetworkError::SocketError(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
