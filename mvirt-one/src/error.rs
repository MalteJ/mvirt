//! Error types for one.

use std::fmt;
use std::io;

/// Main error type for one operations.
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Nix(nix::errno::Errno),
    Mount {
        target: String,
        source: nix::errno::Errno,
    },
    Network(NetworkError),
    Image(ImageError),
    Container(ContainerError),
    Pod(PodError),
}

/// Network-related errors.
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

/// Image pulling and storage errors.
#[derive(Debug)]
pub enum ImageError {
    Registry(String),
    LayerExtraction(String),
    InvalidReference(String),
    NotFound(String),
    Storage(io::Error),
}

/// Container runtime errors.
#[derive(Debug)]
pub enum ContainerError {
    YoukiCommand(String),
    NotFound(String),
    InvalidState { expected: String, actual: String },
    SpecGeneration(String),
}

/// Pod-level errors.
#[derive(Debug)]
pub enum PodError {
    NotFound(String),
    InvalidState { expected: String, actual: String },
    ContainerFailed { container_id: String, error: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "IO error: {e}"),
            Error::Nix(e) => write!(f, "System error: {e}"),
            Error::Mount { target, source } => write!(f, "Failed to mount {target}: {source}"),
            Error::Network(e) => write!(f, "Network error: {e}"),
            Error::Image(e) => write!(f, "Image error: {e}"),
            Error::Container(e) => write!(f, "Container error: {e}"),
            Error::Pod(e) => write!(f, "Pod error: {e}"),
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

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageError::Registry(msg) => write!(f, "Registry error: {msg}"),
            ImageError::LayerExtraction(msg) => write!(f, "Layer extraction failed: {msg}"),
            ImageError::InvalidReference(msg) => write!(f, "Invalid image reference: {msg}"),
            ImageError::NotFound(msg) => write!(f, "Image not found: {msg}"),
            ImageError::Storage(e) => write!(f, "Storage error: {e}"),
        }
    }
}

impl fmt::Display for ContainerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContainerError::YoukiCommand(msg) => write!(f, "youki command failed: {msg}"),
            ContainerError::NotFound(id) => write!(f, "Container not found: {id}"),
            ContainerError::InvalidState { expected, actual } => {
                write!(
                    f,
                    "Invalid container state: expected {expected}, got {actual}"
                )
            }
            ContainerError::SpecGeneration(msg) => write!(f, "OCI spec generation failed: {msg}"),
        }
    }
}

impl fmt::Display for PodError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PodError::NotFound(id) => write!(f, "Pod not found: {id}"),
            PodError::InvalidState { expected, actual } => {
                write!(f, "Invalid pod state: expected {expected}, got {actual}")
            }
            PodError::ContainerFailed {
                container_id,
                error,
            } => {
                write!(f, "Container {container_id} failed: {error}")
            }
        }
    }
}

impl std::error::Error for Error {}
impl std::error::Error for NetworkError {}
impl std::error::Error for ImageError {}
impl std::error::Error for ContainerError {}
impl std::error::Error for PodError {}

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

impl From<ImageError> for Error {
    fn from(e: ImageError) -> Self {
        Error::Image(e)
    }
}

impl From<ContainerError> for Error {
    fn from(e: ContainerError) -> Self {
        Error::Container(e)
    }
}

impl From<PodError> for Error {
    fn from(e: PodError) -> Self {
        Error::Pod(e)
    }
}

impl From<io::Error> for NetworkError {
    fn from(e: io::Error) -> Self {
        NetworkError::SocketError(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
