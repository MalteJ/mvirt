pub mod audit;
pub mod grpc;
pub mod hugepage;
pub mod inter_reactor;
pub mod messaging;
pub mod ping;
pub mod reactor;
pub mod router;
pub mod routing;
pub mod test_util;
pub mod tun;
pub mod vhost_user;
pub mod virtqueue;

// Re-export tonic for external tests that need matching versions
pub use tonic;
